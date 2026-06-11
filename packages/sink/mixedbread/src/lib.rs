//! Mixedbread sink for the multi-source corpus: reconcile a record source
//! (Slack, Linear, Claude, Codex, atuin, git, ...) into a Mixedbread store.
//!
//! Records are addressed by a source-defined `external_id`, so change detection
//! compares each document's `content_hash` against the value stored under that
//! id: upload the new or changed, skip the unchanged. Listing is scoped with a
//! `source == X` filter, and the scope is verified against each returned
//! record's own `source` before anything acts on the listing — a backend that
//! drops the filter aborts the pass instead of feeding a store-wide delete set
//! into [`MixedbreadReconciler::replace`].
//!
//! This is the write half of the corpus, paired with `sink-parquet`. It is built
//! on `search-core`'s [`Store`] abstraction (so it works against the production
//! `MixedbreadStore` or the in-memory test store) and reuses
//! [`search_core::wait_until_indexed`] to block until new content is embedded.

#![forbid(unsafe_code)]

mod error;

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use futures::stream::{self, StreamExt as _};
use mixedbread::Filter;
use search_core::{Store, wait_until_indexed};
use snafu::ResultExt as _;
use source_meta::{Document, Reconciler, Source, SourceAdapter, keys};

pub use crate::error::Error;
use crate::error::{AdapterSnafu, Result, ScopeLeakSnafu, StoreSnafu};

/// Maximum concurrent uploads in flight.
const UPLOAD_CONCURRENCY: usize = 16;

/// Outcome of a reconcile pass over one record source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncReport {
    /// Records uploaded this run (new or changed).
    pub uploaded: usize,
    /// Records skipped because their `content_hash` was unchanged.
    pub skipped: usize,
    /// Total records the adapter produced.
    pub total: usize,
}

/// Outcome of applying a log-derived delta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApplyReport {
    /// Documents uploaded (the delta's upserts).
    pub uploaded: usize,
    /// Records deleted (the delta's tombstones that still existed remotely).
    pub deleted: usize,
}

/// Outcome of a replace pass over one record source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplaceReport {
    /// Records uploaded this run (new or changed).
    pub uploaded: usize,
    /// Records skipped because their `content_hash` was unchanged.
    pub skipped: usize,
    /// Records deleted (present remotely, absent from the desired set).
    pub deleted: usize,
    /// Total records in the desired set.
    pub total: usize,
}

/// Outcome of a garbage-collection pass over one record source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GcReport {
    /// Records deleted (present remotely, absent from the export).
    pub deleted: usize,
    /// Records kept (the adapter's current desired set).
    pub kept: usize,
}

/// The filter selecting one source's records in the shared store.
fn source_filter(source: &Source) -> Filter {
    Filter::eq(keys::SOURCE, source.as_str())
}

/// Reconciles one source's documents into a Mixedbread store.
///
/// Uploads records that are new or whose `content_hash` changed, skips the
/// unchanged, and blocks until the new content is embedded. In a
/// [`Reconciler::reconcile`] pass absent records are kept (deletion is
/// [`gc_documents`], a separate explicit pass); a [`Self::replace`] pass
/// deletes them, for callers replaying a log whose absences are authoritative
/// tombstone folds.
pub struct MixedbreadReconciler<'a, S> {
    /// The store reconciled into (production: `search-core`'s `MixedbreadStore`,
    /// tests: its `MemoryStore`).
    pub store: &'a S,
    /// The store name to sync into.
    pub name: &'a str,
    /// How long to wait for newly uploaded content to be embedded.
    pub index_timeout: Duration,
}

// Manual impls: a derive would needlessly bound `S: Copy`/`S: Clone`, but the
// fields (a shared reference, a str reference, a Duration) are copyable for
// any `S`.
impl<S> Clone for MixedbreadReconciler<'_, S> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<S> Copy for MixedbreadReconciler<'_, S> {}

/// What [`MixedbreadReconciler::sync_source`] saw and did: the remote records
/// listed before uploading, and the upload tallies.
struct SyncOutcome {
    /// Each external id the store held for the source before this pass, with
    /// its stored content hash (`None` predates hash tracking).
    remote: HashMap<String, Option<String>>,
    /// Records uploaded (new or changed).
    uploaded: usize,
    /// Records skipped because their `content_hash` was unchanged.
    skipped: usize,
}

impl<S: Store + Sync> MixedbreadReconciler<'_, S> {
    /// One source's upload half, shared by [`Reconciler::reconcile`] (which
    /// keeps remote absences) and [`Self::replace`] (which deletes them): list
    /// the source's remote records, upload the new or changed documents, and
    /// block until new content is embedded.
    async fn sync_source(&self, source: &Source, documents: &[Document]) -> Result<SyncOutcome> {
        self.store
            .ensure_store(self.name)
            .await
            .context(StoreSnafu)?;
        let filter = source_filter(source);
        let records = self
            .store
            .list_records(self.name, Some(&filter))
            .await
            .context(StoreSnafu)?;

        // Trust but verify the scope before anything derives from the listing.
        // A backend that drops the filter hands back the whole store, and a
        // replace pass would then "delete" every other source's records (it
        // happened: the API renamed the list filter parameter and ignored the
        // old name). Refusing here turns that into a loud no-op instead.
        let mut foreign = records
            .iter()
            .filter(|record| record.source.as_deref() != Some(source.as_str()));
        if let Some(example) = foreign.next() {
            return ScopeLeakSnafu {
                scope: source.as_str().to_owned(),
                count: foreign.count() + 1,
                example: example.external_id.clone(),
            }
            .fail();
        }

        let remote: HashMap<String, Option<String>> = records
            .into_iter()
            .map(|record| (record.external_id, record.content_hash))
            .collect();

        // Uploading is the expensive part and runs concurrently. Only documents
        // that actually need uploading are cloned and held, so reconciling an
        // unchanged corpus holds almost nothing.
        let to_upload: Vec<Document> = documents
            .iter()
            .filter(|document| {
                // A record with no stored content_hash predates hash tracking;
                // re-embed it.
                !matches!(
                    remote.get(&document.external_id),
                    Some(Some(stored)) if *stored == document.content_hash
                )
            })
            .cloned()
            .collect();

        let skipped = documents.len() - to_upload.len();

        let results: Vec<Result<()>> = stream::iter(to_upload)
            .map(|document| async move {
                self.store
                    .upload(self.name, document)
                    .await
                    .context(StoreSnafu)?;
                Ok(())
            })
            .buffer_unordered(UPLOAD_CONCURRENCY)
            .collect()
            .await;

        let mut uploaded = 0;
        for result in results {
            result?;
            uploaded += 1;
        }

        if uploaded > 0 {
            wait_until_indexed(self.store, self.name, self.index_timeout, |_| {})
                .await
                .context(StoreSnafu)?;
        }

        Ok(SyncOutcome {
            remote,
            uploaded,
            skipped,
        })
    }

    /// Make the store's records for one source exactly `documents`: upload the
    /// new or changed, skip the unchanged, and delete remote records absent
    /// from the desired set.
    ///
    /// This is the log-replay sibling of [`Reconciler::reconcile`]. A reconcile
    /// pass scans a live source whose read can be transiently empty or partial,
    /// so absence there is kept; a replace pass replays a durable log fold,
    /// where absence is an explicit tombstone, so absence here is authoritative
    /// — including an empty `documents` for a fully tombstoned source.
    ///
    /// # Errors
    /// Returns an error if the store cannot be reached, an upload fails, or a
    /// delete fails.
    pub async fn replace(&self, source: &Source, documents: &[Document]) -> Result<ReplaceReport> {
        let outcome = self.sync_source(source, documents).await?;
        let desired: HashSet<&str> = documents
            .iter()
            .map(|document| document.external_id.as_str())
            .collect();
        let mut deleted = 0;
        for external_id in outcome.remote.keys() {
            if !desired.contains(external_id.as_str()) {
                self.store
                    .delete(self.name, external_id)
                    .await
                    .context(StoreSnafu)?;
                deleted += 1;
            }
        }
        Ok(ReplaceReport {
            uploaded: outcome.uploaded,
            skipped: outcome.skipped,
            deleted,
            total: documents.len(),
        })
    }

    /// Apply a log-derived delta: upload the changed documents, then delete
    /// the tombstoned ids that still exist in the store.
    ///
    /// Unlike [`Reconciler::reconcile`], this trusts the log's change
    /// detection: no remote listing for skip decisions, every upsert uploads.
    /// Idempotent by construction, so a crash between an apply and its cursor
    /// write replays safely: re-uploading a document overwrites in place, and
    /// deletes are filtered against the store's current ids first (the
    /// production store hard-errors deleting a missing id, which would
    /// otherwise wedge a replayed cursor in a permanent retry loop).
    ///
    /// # Errors
    /// Returns an error if the store cannot be reached, an upload fails, or a
    /// delete of a still-existing record fails.
    pub async fn apply(&self, upserts: Vec<Document>, deletes: &[String]) -> Result<ApplyReport> {
        self.store
            .ensure_store(self.name)
            .await
            .context(StoreSnafu)?;

        let results: Vec<Result<()>> = stream::iter(upserts)
            .map(|document| async move {
                self.store
                    .upload(self.name, document)
                    .await
                    .context(StoreSnafu)?;
                Ok(())
            })
            .buffer_unordered(UPLOAD_CONCURRENCY)
            .collect()
            .await;
        let mut uploaded = 0;
        for result in results {
            result?;
            uploaded += 1;
        }
        if uploaded > 0 {
            wait_until_indexed(self.store, self.name, self.index_timeout, |_| {})
                .await
                .context(StoreSnafu)?;
        }

        let mut removed = 0;
        if !deletes.is_empty() {
            let existing: HashSet<String> = self
                .store
                .list_external_ids(self.name, None)
                .await
                .context(StoreSnafu)?;
            for external_id in deletes {
                if existing.contains(external_id) {
                    self.store
                        .delete(self.name, external_id)
                        .await
                        .context(StoreSnafu)?;
                    removed += 1;
                }
            }
        }

        Ok(ApplyReport {
            uploaded,
            deleted: removed,
        })
    }
}

impl<S: Store + Sync> Reconciler for MixedbreadReconciler<'_, S> {
    type Report = SyncReport;
    type Error = Error;

    /// Upload the new or changed (keyed on `external_id` + `content_hash`),
    /// skip the unchanged, and block until new content is embedded.
    async fn reconcile(&self, source: &Source, documents: &[Document]) -> Result<SyncReport> {
        let outcome = self.sync_source(source, documents).await?;
        Ok(SyncReport {
            uploaded: outcome.uploaded,
            skipped: outcome.skipped,
            total: documents.len(),
        })
    }
}

/// Delete records present in the store for this source but absent from the
/// adapter's current desired set (a full-snapshot set-difference).
///
/// The remote set is listed with a `source == X` filter, so this can only delete
/// that source's records. Run it against a complete export, never a window
/// slice, or it would delete records the slice simply did not include.
///
/// # Errors
/// Returns an error if the store cannot be reached, a document cannot be
/// produced, or a delete fails.
pub async fn gc_documents<A>(
    adapter: &A,
    store: &(impl Store + Sync),
    store_name: &str,
) -> Result<GcReport>
where
    A: SourceAdapter + Sync,
{
    let filter = source_filter(&adapter.source());
    let remote: HashSet<String> = store
        .list_records(store_name, Some(&filter))
        .await
        .context(StoreSnafu)?
        .into_iter()
        .map(|record| record.external_id)
        .collect();

    let mut desired = HashSet::new();
    for item in adapter.documents() {
        let document = item.map_err(|error| {
            AdapterSnafu {
                message: error.to_string(),
            }
            .build()
        })?;
        desired.insert(document.external_id);
    }

    let stale: Vec<&String> = remote.difference(&desired).collect();
    let deleted = stale.len();
    for external_id in stale {
        store
            .delete(store_name, external_id)
            .await
            .context(StoreSnafu)?;
    }

    Ok(GcReport {
        deleted,
        kept: desired.len(),
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use search_core::MemoryStore;
    use source_meta::{Document, Reconciler as _, Source, SourceAdapter};

    use super::{MixedbreadReconciler, gc_documents};

    /// The reconciler under test, with the embedding wait kept short.
    fn reconciler<'a>(
        store: &'a MemoryStore,
        name: &'a str,
    ) -> MixedbreadReconciler<'a, MemoryStore> {
        MixedbreadReconciler {
            store,
            name,
            index_timeout: Duration::from_secs(1),
        }
    }

    // A fake record source for exercising the reconcile and GC without a real
    // parser crate. It yields Linear-shaped documents from owned data.
    struct FakeSource {
        docs: Vec<Document>,
    }

    #[derive(Debug, snafu::Snafu)]
    #[snafu(display("fake source error"))]
    struct FakeError;

    impl SourceAdapter for FakeSource {
        type Error = FakeError;
        fn source(&self) -> source_meta::Source {
            source_meta::Source::new("linear")
        }
        fn documents(
            &self,
        ) -> impl Iterator<Item = std::result::Result<Document, FakeError>> + Send {
            self.docs.clone().into_iter().map(Ok)
        }
    }

    fn linear_doc(id: &str, body: &str) -> Document {
        let content_hash = source_meta::hash_body(body.as_bytes());
        Document {
            external_id: format!("linear:issue:{id}"),
            file_name: format!("{id}.txt"),
            mime: "text/plain",
            body: body.as_bytes().to_vec(),
            meta_json: serde_json::json!({
                "source": "linear",
                "external_id": format!("linear:issue:{id}"),
                "content_hash": content_hash,
                "title": id,
            }),
            content_hash,
        }
    }

    #[tokio::test]
    async fn document_sync_uploads_then_skips_unchanged_and_reuploads_changed() {
        let store = MemoryStore::new();
        let sink = reconciler(&store, "s");
        let source = Source::new("linear");
        let docs = vec![linear_doc("A", "alpha body"), linear_doc("B", "beta body")];

        let first = sink.reconcile(&source, &docs).await.expect("first");
        assert_eq!(first.uploaded, 2);
        assert_eq!(store.upload_count(), 2);

        // Re-running the same export uploads nothing (content_hash unchanged).
        let second = sink.reconcile(&source, &docs).await.expect("second");
        assert_eq!(second.uploaded, 0);
        assert_eq!(second.skipped, 2);
        assert_eq!(store.upload_count(), 2, "no redundant re-upload");

        // A changed body for A re-embeds only A.
        let changed = vec![
            linear_doc("A", "alpha body EDITED"),
            linear_doc("B", "beta body"),
        ];
        let third = sink.reconcile(&source, &changed).await.expect("third");
        assert_eq!(third.uploaded, 1);
        assert_eq!(store.upload_count(), 3);
    }

    #[tokio::test]
    async fn apply_delta_uploads_and_deletes_idempotently() {
        let store = MemoryStore::new();
        let sink = reconciler(&store, "s");
        let source = Source::new("linear");
        sink.reconcile(&source, &[linear_doc("A", "a"), linear_doc("B", "b")])
            .await
            .expect("seed");

        // A delta: A changed, B tombstoned, C never existed (a replayed delete).
        let delta_upserts = vec![linear_doc("A", "a EDITED")];
        let deletes = vec!["linear:issue:B".to_owned(), "linear:issue:C".to_owned()];
        let report = sink
            .apply(delta_upserts.clone(), &deletes)
            .await
            .expect("apply");
        assert_eq!(report.uploaded, 1);
        assert_eq!(
            report.deleted, 1,
            "the never-existed id must be skipped, not an error"
        );
        assert_eq!(store.len(), 1, "only A remains");

        // Replaying the same delta (a crash before the cursor write) is safe:
        // the re-upload overwrites in place and the delete finds nothing.
        let replay = sink.apply(delta_upserts, &deletes).await.expect("replay");
        assert_eq!(replay.uploaded, 1);
        assert_eq!(replay.deleted, 0);
        assert_eq!(store.len(), 1);
    }

    #[tokio::test]
    async fn replace_deletes_absences_and_scopes_to_the_source() {
        let store = MemoryStore::new();
        let sink = reconciler(&store, "s");
        let linear = Source::new("linear");
        sink.reconcile(&linear, &[linear_doc("A", "a"), linear_doc("B", "b")])
            .await
            .expect("seed linear");
        // A second source sharing the store must be invisible to the replace.
        let mut other = linear_doc("O", "o");
        other.meta_json["source"] = serde_json::json!("other");
        sink.reconcile(&Source::new("other"), std::slice::from_ref(&other))
            .await
            .expect("seed other");

        // The log fold now holds A (changed) and C; B was tombstoned.
        let desired = vec![linear_doc("A", "a EDITED"), linear_doc("C", "c")];
        let report = sink.replace(&linear, &desired).await.expect("replace");
        assert_eq!(
            report.uploaded, 2,
            "the changed and the new document upload"
        );
        assert_eq!(report.skipped, 0);
        assert_eq!(report.deleted, 1, "the absent document is deleted");
        assert_eq!(report.total, 2);
        assert_eq!(store.len(), 3, "linear A+C survive, other O untouched");

        // Replaying the same fold converges: nothing uploads, nothing deletes.
        let again = sink.replace(&linear, &desired).await.expect("replay");
        assert_eq!((again.uploaded, again.skipped, again.deleted), (0, 2, 0));

        // A fully tombstoned source folds to an empty desired set, and that
        // emptiness is authoritative for a replace (unlike reconcile, whose
        // live-scan absences are protective).
        let report = sink.replace(&linear, &[]).await.expect("empty replace");
        assert_eq!(
            report.deleted, 2,
            "an empty fold deletes the source's records"
        );
        assert_eq!(store.len(), 1, "only the other source's record remains");
    }

    /// A store whose record listing drops the requested filter, standing in for
    /// a backend that silently ignores an unrecognized filter parameter (the
    /// production API did exactly this when the parameter was misnamed).
    struct UnscopedStore(MemoryStore);

    impl search_core::Store for UnscopedStore {
        async fn ensure_store(&self, name: &str) -> search_core::Result<()> {
            self.0.ensure_store(name).await
        }
        async fn list_external_ids(
            &self,
            store: &str,
            _filters: Option<&search_core::Filter>,
        ) -> search_core::Result<std::collections::HashSet<String>> {
            self.0.list_external_ids(store, None).await
        }
        async fn list_records(
            &self,
            store: &str,
            _filters: Option<&search_core::Filter>,
        ) -> search_core::Result<Vec<search_core::StoredRecord>> {
            self.0.list_records(store, None).await
        }
        async fn upload(&self, store: &str, document: Document) -> search_core::Result<()> {
            self.0.upload(store, document).await
        }
        async fn delete(&self, store: &str, external_id: &str) -> search_core::Result<()> {
            self.0.delete(store, external_id).await
        }
        async fn search(
            &self,
            stores: &[String],
            query: &str,
            top_k: usize,
            options: search_core::SearchOptions,
            filters: Option<&search_core::Filter>,
        ) -> search_core::Result<Vec<search_core::SearchHit>> {
            self.0.search(stores, query, top_k, options, filters).await
        }
        async fn grep(
            &self,
            stores: &[String],
            pattern: &str,
            top_k: usize,
            options: search_core::GrepOptions,
            filters: Option<&search_core::Filter>,
        ) -> search_core::Result<Vec<search_core::SearchHit>> {
            self.0.grep(stores, pattern, top_k, options, filters).await
        }
        async fn ask(
            &self,
            stores: &[String],
            query: &str,
            top_k: usize,
            options: search_core::SearchOptions,
            filters: Option<&search_core::Filter>,
        ) -> search_core::Result<search_core::Answer> {
            self.0.ask(stores, query, top_k, options, filters).await
        }
        async fn store_status(&self, store: &str) -> search_core::Result<search_core::StoreStatus> {
            self.0.store_status(store).await
        }
    }

    #[tokio::test]
    async fn a_listing_that_leaks_other_sources_aborts_before_any_delete() {
        // Seed two sources through the well-behaved store, then wrap it so
        // listings come back unscoped, as they did from the production API.
        let memory = MemoryStore::new();
        reconciler(&memory, "s")
            .reconcile(&Source::new("linear"), &[linear_doc("A", "a")])
            .await
            .expect("seed linear");
        let mut other = linear_doc("O", "o");
        other.meta_json["source"] = serde_json::json!("other");
        reconciler(&memory, "s")
            .reconcile(&Source::new("other"), std::slice::from_ref(&other))
            .await
            .expect("seed other");

        let store = UnscopedStore(memory);
        let err = MixedbreadReconciler {
            store: &store,
            name: "s",
            index_timeout: Duration::from_secs(1),
        }
        .replace(&Source::new("linear"), &[linear_doc("A", "a")])
        .await
        .expect_err("an unscoped listing must abort the replace");
        assert!(
            matches!(err, crate::Error::ScopeLeak { .. }),
            "got {err:?}"
        );
        assert_eq!(
            store.0.len(),
            2,
            "nothing may be uploaded or deleted off a leaked listing"
        );
    }

    #[tokio::test]
    async fn gc_deletes_records_absent_from_the_export() {
        let store = MemoryStore::new();
        let docs = vec![
            linear_doc("A", "a"),
            linear_doc("B", "b"),
            linear_doc("C", "c"),
        ];
        reconciler(&store, "s")
            .reconcile(&Source::new("linear"), &docs)
            .await
            .expect("seed");
        assert_eq!(store.len(), 3);

        // A later export dropped issue C; GC removes it.
        let trimmed = FakeSource {
            docs: vec![linear_doc("A", "a"), linear_doc("B", "b")],
        };
        let report = gc_documents(&trimmed, &store, "s").await.expect("gc");
        assert_eq!(report.deleted, 1);
        assert_eq!(report.kept, 2);
        assert_eq!(store.len(), 2);
    }
}
