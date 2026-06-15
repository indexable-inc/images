//! Mixedbread sink for the multi-source corpus: reconcile a record source
//! (Slack, Linear, Claude, Codex, atuin, git, ...) into a Mixedbread store.
//!
//! Records are addressed by a source-defined `external_id`, so change detection
//! compares each document's `content_hash` against the value stored under that
//! id: upload the new or changed, skip the unchanged. Listing is scoped with a
//! `source == X` filter, and the scope is verified against each returned
//! record's own `source` before anything acts on the listing — a backend that
//! drops the filter aborts the pass instead of feeding a store-wide delete set
//! into [`MixedbreadReconciler::replace`] or [`MixedbreadReconciler::gc`].
//!
//! This is the write half of the corpus, paired with `sink-parquet`. It is built
//! on `search-core`'s [`Store`] abstraction (so it works against the production
//! `MixedbreadStore` or the in-memory test store) and reuses
//! [`search_core::wait_until_indexed`] to block until new content is embedded.

#![forbid(unsafe_code)]

mod error;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Duration;

use futures::stream::{self, StreamExt as _};
use mixedbread::Filter;
use search_core::{Store, StoredRecord, wait_until_indexed};
use snafu::ResultExt as _;
use source_meta::{Document, Reconciler, Source, keys};

pub use crate::error::Error;
use crate::error::{Result, ScopeLeakSnafu, StoreSnafu};

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
    /// File objects deleted because their `external_id` vanished from the
    /// produced set (present remotely, absent from the export).
    pub deleted: usize,
    /// Exact-duplicate `(external_id, content_hash)` file objects deleted,
    /// the newest of each group kept (the residue of retried uploads).
    pub deduped: usize,
    /// External ids kept (the source's current desired set).
    pub kept: usize,
}

/// The filter selecting one source's records in the shared store.
fn source_filter(source: &Source) -> Filter {
    Filter::eq(keys::SOURCE, source.as_str())
}

/// Trust but verify a `source == X`-scoped listing before anything derives
/// from it. A backend that drops the filter hands back the whole store, and a
/// replace or GC pass would then "delete" every other source's records (it
/// happened: the API renamed the list filter parameter and ignored the old
/// name). Refusing here turns that into a loud no-op instead.
fn verify_scope(source: &Source, records: &[StoredRecord]) -> Result<()> {
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
    Ok(())
}

/// Reconciles one source's documents into a Mixedbread store.
///
/// Uploads records that are new or whose `content_hash` changed, skips the
/// unchanged, and blocks until the new content is embedded. In a
/// [`Reconciler::reconcile`] pass absent records are kept (deletion is
/// [`Self::gc`], a separate explicit pass); a [`Self::replace`] pass
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

        verify_scope(source, &records)?;

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

        // The embedding wait below is gated on exactly these ids, never the
        // store-wide pending counts: the store is shared by every source, so
        // another source's backlog must not block this source's pass (ENG-2699).
        let uploaded_ids: Vec<String> = to_upload
            .iter()
            .map(|document| document.external_id.clone())
            .collect();

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
            wait_until_indexed(self.store, self.name, &uploaded_ids, self.index_timeout, |_| {})
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

        // As in `sync_source`: wait on this delta's own ids, not the shared
        // store's aggregate backlog (ENG-2699).
        let upserted_ids: Vec<String> = upserts
            .iter()
            .map(|document| document.external_id.clone())
            .collect();

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
            wait_until_indexed(self.store, self.name, &upserted_ids, self.index_timeout, |_| {})
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

    /// Garbage-collect one source after a fully successful sync: delete file
    /// objects whose `external_id` is absent from `produced`, and
    /// exact-duplicate `(external_id, content_hash)` file objects left behind
    /// by retried uploads, keeping the newest of each group.
    ///
    /// `produced` must be the COMPLETE external-id set the source emitted this
    /// run (an export directory is complete by construction; a live scan that
    /// can be transiently partial is not) — against a partial set this would
    /// delete records the input simply did not include. The listing is scoped
    /// with a `source == X` filter and verified the same way a replace pass
    /// is, so a backend that drops the filter aborts loudly instead of feeding
    /// a store-wide delete set in.
    ///
    /// # Errors
    /// Returns an error if the store cannot be reached, the scoped listing
    /// leaks foreign records, or a delete fails.
    pub async fn gc(&self, source: &Source, produced: &HashSet<String>) -> Result<GcReport> {
        let filter = source_filter(source);
        let records = self
            .store
            .list_records(self.name, Some(&filter))
            .await
            .context(StoreSnafu)?;
        verify_scope(source, &records)?;

        let plan = plan_gc(&records, produced);
        for key in &plan.deletes {
            self.store.delete(self.name, key).await.context(StoreSnafu)?;
        }

        Ok(GcReport {
            deleted: plan.vanished,
            deduped: plan.duplicates,
            kept: produced.len(),
        })
    }
}

/// The deletions one GC pass will perform, derived purely from the scoped
/// listing and the produced id set so the policy is testable without a store.
#[derive(Debug, PartialEq, Eq)]
struct GcPlan {
    /// Delete keys in deletion order: the file object's own id when the
    /// listing carried one, else the external id. Vanished records come
    /// before duplicate trims.
    deletes: Vec<String>,
    /// How many of `deletes` are vanished records.
    vanished: usize,
    /// How many of `deletes` are duplicate file objects.
    duplicates: usize,
}

/// Decide what a GC pass deletes (see [`MixedbreadReconciler::gc`]).
///
/// Two rules, both conservative:
///
/// - An `external_id` absent from `produced` vanished from the export: every
///   file object under it is deleted (by its own file id; a record the
///   backend reported without one falls back to one external-id delete, which
///   is ambiguous between duplicates but safe when all of them are condemned).
/// - A surviving `external_id` is trimmed to one file object per
///   `content_hash`: an exact `(external_id, content_hash)` twin is the
///   residue of a retried `POST /v1/files` and only the newest (by
///   `created_at`, then file id; a missing timestamp sorts oldest) is kept.
///   Two hashes under one id are NOT twins — one of them is the current
///   version — and a twin without a file id is left alone rather than risk an
///   ambiguous delete hitting the keeper.
fn plan_gc(records: &[StoredRecord], produced: &HashSet<String>) -> GcPlan {
    // BTreeMaps keep the plan deterministic for a given listing order.
    let mut groups: BTreeMap<&str, Vec<&StoredRecord>> = BTreeMap::new();
    for record in records {
        groups
            .entry(record.external_id.as_str())
            .or_default()
            .push(record);
    }

    let mut deletes = Vec::new();
    let mut vanished = 0;
    let mut duplicates = 0;
    for (external_id, group) in groups {
        if !produced.contains(external_id) {
            let mut deleted_by_external_id = false;
            for record in &group {
                match &record.file_id {
                    Some(file_id) => deletes.push(file_id.clone()),
                    None if deleted_by_external_id => continue,
                    None => {
                        deletes.push(external_id.to_owned());
                        deleted_by_external_id = true;
                    }
                }
                vanished += 1;
            }
            continue;
        }

        let mut by_hash: BTreeMap<&str, Vec<&StoredRecord>> = BTreeMap::new();
        for record in group {
            if let Some(hash) = record.content_hash.as_deref() {
                by_hash.entry(hash).or_default().push(record);
            }
        }
        for mut twins in by_hash.into_values() {
            if twins.len() < 2 {
                continue;
            }
            // Newest last: RFC 3339 timestamps in one zone order
            // lexicographically, `None` (no timestamp) sorts oldest, and the
            // file id breaks ties deterministically.
            twins.sort_by_key(|record| (record.created_at.as_deref(), record.file_id.as_deref()));
            let Some((_newest, condemned)) = twins.split_last() else {
                continue;
            };
            for record in condemned {
                if let Some(file_id) = &record.file_id {
                    deletes.push(file_id.clone());
                    duplicates += 1;
                }
            }
        }
    }

    GcPlan {
        deletes,
        vanished,
        duplicates,
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

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::time::Duration;

    use search_core::{MemoryStore, StoredRecord};
    use source_meta::{Document, Reconciler as _, Source};

    use super::{MixedbreadReconciler, plan_gc};

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
        async fn list_chunks(
            &self,
            stores: &[String],
            top_k: usize,
            filters: Option<&search_core::Filter>,
            sort_by: Option<&search_core::SortBy>,
        ) -> search_core::Result<Vec<search_core::SearchHit>> {
            self.0.list_chunks(stores, top_k, filters, sort_by).await
        }
        async fn facets(
            &self,
            stores: &[String],
            keys: &[&str],
            filters: Option<&search_core::Filter>,
        ) -> search_core::Result<search_core::Facets> {
            self.0.facets(stores, keys, filters).await
        }
        async fn ask(
            &self,
            stores: &[String],
            query: &str,
            top_k: usize,
            options: search_core::AskOptions,
            filters: Option<&search_core::Filter>,
        ) -> search_core::Result<search_core::Answer> {
            self.0.ask(stores, query, top_k, options, filters).await
        }
        async fn store_status(&self, store: &str) -> search_core::Result<search_core::StoreStatus> {
            self.0.store_status(store).await
        }
        async fn file_status(
            &self,
            store: &str,
            external_id: &str,
        ) -> search_core::Result<Option<search_core::FileStatus>> {
            self.0.file_status(store, external_id).await
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

    /// A store whose aggregate counts report a permanent backlog (another
    /// source's documents, forever embedding) while every file actually stored
    /// is already settled. Old behavior gated each source's post-upload wait on
    /// the aggregate, so this store would stall every reconcile until its full
    /// `index_timeout` elapsed.
    struct BackloggedStore(MemoryStore);

    impl search_core::Store for BackloggedStore {
        async fn ensure_store(&self, name: &str) -> search_core::Result<()> {
            self.0.ensure_store(name).await
        }
        async fn list_external_ids(
            &self,
            store: &str,
            filters: Option<&search_core::Filter>,
        ) -> search_core::Result<std::collections::HashSet<String>> {
            self.0.list_external_ids(store, filters).await
        }
        async fn list_records(
            &self,
            store: &str,
            filters: Option<&search_core::Filter>,
        ) -> search_core::Result<Vec<search_core::StoredRecord>> {
            self.0.list_records(store, filters).await
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
        async fn list_chunks(
            &self,
            stores: &[String],
            top_k: usize,
            filters: Option<&search_core::Filter>,
            sort_by: Option<&search_core::SortBy>,
        ) -> search_core::Result<Vec<search_core::SearchHit>> {
            self.0.list_chunks(stores, top_k, filters, sort_by).await
        }
        async fn facets(
            &self,
            stores: &[String],
            keys: &[&str],
            filters: Option<&search_core::Filter>,
        ) -> search_core::Result<search_core::Facets> {
            self.0.facets(stores, keys, filters).await
        }
        async fn ask(
            &self,
            stores: &[String],
            query: &str,
            top_k: usize,
            options: search_core::AskOptions,
            filters: Option<&search_core::Filter>,
        ) -> search_core::Result<search_core::Answer> {
            self.0.ask(stores, query, top_k, options, filters).await
        }
        async fn store_status(&self, _store: &str) -> search_core::Result<search_core::StoreStatus> {
            Ok(search_core::StoreStatus {
                pending: 999,
                in_progress: 0,
            })
        }
        async fn file_status(
            &self,
            store: &str,
            external_id: &str,
        ) -> search_core::Result<Option<search_core::FileStatus>> {
            self.0.file_status(store, external_id).await
        }
    }

    #[tokio::test]
    async fn an_unrelated_backlog_does_not_stall_this_sources_wait() {
        // ENG-2699: the post-upload embedding wait must be gated on the files
        // THIS reconcile uploaded, not the store-wide pending counts. With a
        // permanent 999-file aggregate backlog and a deliberately long
        // index_timeout, the old store-wide gate would block for the full
        // timeout; the per-file gate returns as soon as our two uploads settle.
        let store = BackloggedStore(MemoryStore::new());
        let sink = MixedbreadReconciler {
            store: &store,
            name: "s",
            index_timeout: Duration::from_mins(1),
        };
        let docs = vec![linear_doc("A", "a"), linear_doc("B", "b")];

        let report = tokio::time::timeout(
            Duration::from_secs(5),
            sink.reconcile(&Source::new("linear"), &docs),
        )
        .await
        .expect("the wait must not be gated on the unrelated backlog")
        .expect("reconcile");
        assert_eq!(report.uploaded, 2);

        // The delta-apply path shares the gate.
        let apply = tokio::time::timeout(
            Duration::from_secs(5),
            sink.apply(vec![linear_doc("A", "a EDITED")], &[]),
        )
        .await
        .expect("apply's wait must not be gated on the unrelated backlog")
        .expect("apply");
        assert_eq!(apply.uploaded, 1);
    }

    #[tokio::test]
    async fn gc_deletes_records_absent_from_the_export_and_spares_other_sources() {
        let store = MemoryStore::new();
        let docs = vec![
            linear_doc("A", "a"),
            linear_doc("B", "b"),
            linear_doc("C", "c"),
        ];
        let linear = Source::new("linear");
        reconciler(&store, "s")
            .reconcile(&linear, &docs)
            .await
            .expect("seed");
        // A second source sharing the store must be invisible to the GC.
        let mut other = linear_doc("O", "o");
        other.meta_json["source"] = serde_json::json!("other");
        reconciler(&store, "s")
            .reconcile(&Source::new("other"), std::slice::from_ref(&other))
            .await
            .expect("seed other");
        assert_eq!(store.len(), 4);

        // A later export dropped issue C; GC removes it and nothing else.
        let produced: HashSet<String> = ["linear:issue:A", "linear:issue:B"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        let report = reconciler(&store, "s")
            .gc(&linear, &produced)
            .await
            .expect("gc");
        assert_eq!(report.deleted, 1);
        assert_eq!(report.deduped, 0);
        assert_eq!(report.kept, 2);
        assert_eq!(store.len(), 3, "linear A+B survive, other O untouched");

        // Replaying the same GC converges: nothing left to delete.
        let again = reconciler(&store, "s")
            .gc(&linear, &produced)
            .await
            .expect("replay");
        assert_eq!(again.deleted, 0);
        assert_eq!(store.len(), 3);
    }

    #[tokio::test]
    async fn a_leaked_listing_aborts_gc_before_any_delete() {
        // GC derives a delete set from a scoped listing, exactly like replace:
        // a backend that drops the filter must abort it, not have the whole
        // store diffed against one source's export.
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
        let produced: HashSet<String> = std::iter::once("linear:issue:A".to_owned()).collect();
        let err = MixedbreadReconciler {
            store: &store,
            name: "s",
            index_timeout: Duration::from_secs(1),
        }
        .gc(&Source::new("linear"), &produced)
        .await
        .expect_err("an unscoped listing must abort the GC");
        assert!(matches!(err, crate::Error::ScopeLeak { .. }), "got {err:?}");
        assert_eq!(store.0.len(), 2, "nothing may be deleted off a leaked listing");
    }

    /// A listed record with every identity field the GC policy reads.
    fn stored(
        external_id: &str,
        hash: &str,
        file_id: Option<&str>,
        created_at: Option<&str>,
    ) -> StoredRecord {
        StoredRecord {
            external_id: external_id.to_owned(),
            content_hash: Some(hash.to_owned()),
            source: Some("linear".to_owned()),
            file_id: file_id.map(str::to_owned),
            created_at: created_at.map(str::to_owned),
        }
    }

    fn produced(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|id| (*id).to_owned()).collect()
    }

    #[test]
    fn plan_gc_deletes_every_file_object_of_a_vanished_id() {
        // `B` vanished from the export and (after a retried upload) holds two
        // file objects: both are condemned, each by its own file id. `A`
        // survives untouched.
        let records = [
            stored("A", "sha256:aa", Some("f-a"), Some("2026-06-10T00:00:00Z")),
            stored("B", "sha256:bb", Some("f-b1"), Some("2026-06-10T00:00:00Z")),
            stored("B", "sha256:bb", Some("f-b2"), Some("2026-06-11T00:00:00Z")),
        ];
        let plan = plan_gc(&records, &produced(&["A"]));
        assert_eq!(plan.deletes, vec!["f-b1".to_owned(), "f-b2".to_owned()]);
        assert_eq!(plan.vanished, 2);
        assert_eq!(plan.duplicates, 0);
    }

    #[test]
    fn plan_gc_falls_back_to_one_external_id_delete_without_file_ids() {
        // A backend that reports no per-object file id (the in-memory store)
        // still gets its vanished record deleted, addressed by external id —
        // and only once, however the group is shaped.
        let records = [stored("B", "sha256:bb", None, None)];
        let plan = plan_gc(&records, &produced(&[]));
        assert_eq!(plan.deletes, vec!["B".to_owned()]);
        assert_eq!(plan.vanished, 1);
    }

    #[test]
    fn plan_gc_trims_exact_duplicates_keeping_the_newest() {
        // ENG-2702: a retried `POST /v1/files` left three file objects under
        // one surviving (external_id, content_hash). Only the newest stays; a
        // missing created_at sorts oldest, so the undated twin is condemned
        // ahead of the dated ones.
        let records = [
            stored("A", "sha256:aa", Some("f-old"), Some("2026-06-10T00:00:00Z")),
            stored("A", "sha256:aa", Some("f-new"), Some("2026-06-11T09:30:00Z")),
            stored("A", "sha256:aa", Some("f-undated"), None),
        ];
        let plan = plan_gc(&records, &produced(&["A"]));
        assert_eq!(plan.deletes, vec!["f-undated".to_owned(), "f-old".to_owned()]);
        assert_eq!(plan.vanished, 0);
        assert_eq!(plan.duplicates, 2);
    }

    #[test]
    fn plan_gc_never_touches_distinct_hashes_or_unaddressable_twins() {
        // Two different hashes under one id are a stale-plus-current pair, not
        // duplicates: reconcile owns that case, GC must not guess which to
        // keep. And an exact twin the backend reported without a file id
        // cannot be addressed individually, so it is left alone rather than
        // risk an ambiguous external-id delete hitting the keeper.
        let records = [
            stored("A", "sha256:old", Some("f-1"), Some("2026-06-10T00:00:00Z")),
            stored("A", "sha256:new", Some("f-2"), Some("2026-06-11T00:00:00Z")),
            stored("B", "sha256:bb", None, None),
            stored("B", "sha256:bb", Some("f-b"), Some("2026-06-11T00:00:00Z")),
        ];
        let plan = plan_gc(&records, &produced(&["A", "B"]));
        assert_eq!(
            plan.deletes,
            Vec::<String>::new(),
            "neither group holds an addressable exact duplicate"
        );
    }

    #[test]
    fn plan_gc_with_an_empty_produced_set_condemns_the_whole_source() {
        // The complete-input contract cuts both ways: an empty export is an
        // authoritative "this source now has nothing", mirroring replace's
        // empty-fold semantics. Callers gate on input completeness, not here.
        let records = [
            stored("A", "sha256:aa", Some("f-a"), None),
            stored("B", "sha256:bb", Some("f-b"), None),
        ];
        let plan = plan_gc(&records, &produced(&[]));
        assert_eq!(plan.deletes, vec!["f-a".to_owned(), "f-b".to_owned()]);
        assert_eq!(plan.vanished, 2);
    }
}
