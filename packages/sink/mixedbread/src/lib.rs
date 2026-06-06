//! Mixedbread sink for the multi-source corpus: reconcile a record source
//! (Slack, Linear, Claude, Codex, atuin, git, ...) into a Mixedbread store.
//!
//! Records are addressed by a source-defined `external_id`, so change detection
//! compares each document's `content_hash` against the value stored under that
//! id: upload the new or changed, skip the unchanged. Listing is scoped with a
//! `source == X` filter, so a reconcile never reads or touches another source's
//! records.
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
use source_meta::{Document, Reconciler, Source, SourceAdapter, keys};
use snafu::ResultExt as _;

pub use crate::error::Error;
use crate::error::{AdapterSnafu, Result, StoreSnafu};

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
/// unchanged, and blocks until the new content is embedded. Absent records are
/// kept — deletion is [`gc_documents`], a separate explicit pass.
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

impl<S: Store + Sync> Reconciler for MixedbreadReconciler<'_, S> {
    type Report = SyncReport;
    type Error = Error;

    /// Upload the new or changed (keyed on `external_id` + `content_hash`),
    /// skip the unchanged, and block until new content is embedded.
    async fn reconcile(&self, source: &Source, documents: &[Document]) -> Result<SyncReport> {
        self.store.ensure_store(self.name).await.context(StoreSnafu)?;
        let filter = source_filter(source);
        let remote: HashMap<String, Option<String>> = self
            .store
            .list_records(self.name, Some(&filter))
            .await
            .context(StoreSnafu)?
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

        let total = documents.len();
        let skipped = total - to_upload.len();

        let results: Vec<Result<()>> = stream::iter(to_upload)
            .map(|document| async move {
                self.store.upload(self.name, document).await.context(StoreSnafu)?;
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

        Ok(SyncReport { uploaded, skipped, total })
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
        let document = item.map_err(|error| AdapterSnafu { message: error.to_string() }.build())?;
        desired.insert(document.external_id);
    }

    let stale: Vec<&String> = remote.difference(&desired).collect();
    let deleted = stale.len();
    for external_id in stale {
        store.delete(store_name, external_id).await.context(StoreSnafu)?;
    }

    Ok(GcReport { deleted, kept: desired.len() })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use search_core::MemoryStore;
    use source_meta::{Document, Reconciler as _, Source, SourceAdapter};

    use super::{MixedbreadReconciler, gc_documents};

    /// The reconciler under test, with the embedding wait kept short.
    fn reconciler<'a>(store: &'a MemoryStore, name: &'a str) -> MixedbreadReconciler<'a, MemoryStore> {
        MixedbreadReconciler { store, name, index_timeout: Duration::from_secs(1) }
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
        fn documents(&self) -> impl Iterator<Item = std::result::Result<Document, FakeError>> + Send {
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
        let changed = vec![linear_doc("A", "alpha body EDITED"), linear_doc("B", "beta body")];
        let third = sink.reconcile(&source, &changed).await.expect("third");
        assert_eq!(third.uploaded, 1);
        assert_eq!(store.upload_count(), 3);
    }

    #[tokio::test]
    async fn gc_deletes_records_absent_from_the_export() {
        let store = MemoryStore::new();
        let docs = vec![linear_doc("A", "a"), linear_doc("B", "b"), linear_doc("C", "c")];
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
