//! The async upload path: a bounded queue, a worker pool that reconciles each
//! document into Mixedbread, and a dedup cache so a re-delivered record is not
//! re-embedded.
//!
//! The HTTP handler stays fast: it projects records and hands them to [`Ingest`]
//! over a bounded channel, returning backpressure (a full queue) to the caller
//! rather than blocking on Mixedbread, which is network-bound and rate-limited.
//! A drain task pulls from the channel and uploads with bounded concurrency.
//!
//! Dedup is applied at upload time, not enqueue time: an id is recorded as seen
//! only after a successful upload, so a record dropped under backpressure (queue
//! full, the handler 503s and the collector retries) always re-flows, and the
//! dedup cache itself never causes loss. Because the store is keyed by
//! `external_id`, a re-delivery is an idempotent overwrite, so the cache only
//! saves redundant embedding work.
//!
//! Durability is best-effort, by design. The HTTP handler acks (200) on enqueue,
//! before upload, so a record whose upload still fails after `max_attempts`
//! (logged at `error!`) is dropped, and the collector, already told success, does
//! not retry it. A *sustained* outage instead fills the queue so the handler
//! 503s and the collector holds the data; the loss window is the narrow case of
//! per-record failures that exhaust retries while the queue is not saturated.
//! This is acceptable because Mixedbread is a semantic-search index, not the
//! system of record: the same logs are durably retained by the collector's other
//! exporter (e.g. ClickHouse).

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use search_core::{MixedbreadStore, Store as _};
use source_meta::Document;
use tokio::sync::{Semaphore, mpsc};

/// Base delay for the upload retry backoff.
const RETRY_BASE: Duration = Duration::from_millis(500);
/// Cap on a single retry delay.
const RETRY_CAP: Duration = Duration::from_secs(30);

/// Outcome of offering a document to the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sent {
    /// Queued for upload.
    Accepted,
    /// The queue is full; the caller should apply backpressure (retry later).
    Full,
    /// The drain task is gone; the service is shutting down.
    Closed,
}

/// Handle the HTTP layer uses to offer documents to the upload pipeline.
pub struct Ingest {
    tx: mpsc::Sender<Document>,
}

impl Ingest {
    /// Offer one document to the queue without blocking.
    #[must_use]
    pub fn offer(&self, document: Document) -> Sent {
        match self.tx.try_send(document) {
            Ok(()) => Sent::Accepted,
            Err(mpsc::error::TrySendError::Full(_)) => Sent::Full,
            Err(mpsc::error::TrySendError::Closed(_)) => Sent::Closed,
        }
    }
}

/// Tuning for the upload pipeline.
#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// Bounded queue depth between the handler and the workers.
    pub queue_capacity: usize,
    /// Maximum concurrent uploads in flight.
    pub concurrency: usize,
    /// How many recent `external_id`s to remember for dedup.
    pub dedup_capacity: usize,
    /// Upload attempts per document before giving up (and logging).
    pub max_attempts: u32,
}

/// Spawn the drain + worker pipeline and return the [`Ingest`] handle.
///
/// The drain task lives for the process lifetime; it ends only when the channel
/// closes (the handle is dropped at shutdown).
#[must_use]
pub fn spawn(store: Arc<MixedbreadStore>, store_name: Arc<str>, config: Config) -> Arc<Ingest> {
    let (tx, mut rx) = mpsc::channel::<Document>(config.queue_capacity);
    let limiter = Arc::new(Semaphore::new(config.concurrency.max(1)));
    let dedup = Arc::new(Mutex::new(Dedup::new(config.dedup_capacity)));
    let attempts = config.max_attempts.max(1);

    tokio::spawn(async move {
        while let Some(document) = rx.recv().await {
            if locked(&dedup).contains(&document.external_id, &document.content_hash) {
                continue;
            }
            // Bound concurrent uploads; the permit is released when the task ends.
            let permit = limiter
                .clone()
                .acquire_owned()
                .await
                .expect("upload semaphore is never closed while the drain task runs");
            let store = store.clone();
            let store_name = store_name.clone();
            let dedup = dedup.clone();
            tokio::spawn(async move {
                let _permit = permit;
                let id = document.external_id.clone();
                let hash = document.content_hash.clone();
                if upload_with_retry(&store, &store_name, document, attempts).await {
                    locked(&dedup).insert(id, hash);
                }
            });
        }
    });

    Arc::new(Ingest { tx })
}

/// Upload one document, retrying transient failures with exponential backoff.
/// Returns whether it ultimately succeeded; a permanent failure is logged.
async fn upload_with_retry(
    store: &MixedbreadStore,
    store_name: &str,
    document: Document,
    max_attempts: u32,
) -> bool {
    let external_id = document.external_id.clone();
    for attempt in 1..=max_attempts {
        // `upload` consumes the document, so each attempt gets its own clone.
        match store.upload(store_name, document.clone()).await {
            Ok(()) => return true,
            Err(error) => {
                if attempt == max_attempts {
                    tracing::error!(%external_id, %error, attempt, "giving up on log upload");
                    return false;
                }
                tracing::warn!(%external_id, %error, attempt, "log upload failed, retrying");
                tokio::time::sleep(backoff(attempt)).await;
            }
        }
    }
    false
}

/// Exponential backoff for retry `attempt` (1-based), capped at [`RETRY_CAP`].
fn backoff(attempt: u32) -> Duration {
    RETRY_BASE.saturating_mul(1u32 << (attempt - 1).min(16)).min(RETRY_CAP)
}

/// Lock the dedup mutex, recovering the inner value if a prior holder panicked
/// (the critical section is panic-free, so this is just defensive).
fn locked(dedup: &Mutex<Dedup>) -> std::sync::MutexGuard<'_, Dedup> {
    dedup.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A bounded FIFO set of recently-uploaded `external_id` -> `content_hash`.
///
/// `contains` is a true skip only when both the id and its content match, so an
/// updated record (same id, new body) still re-uploads.
struct Dedup {
    cap: usize,
    seen: HashMap<String, String>,
    order: VecDeque<String>,
}

impl Dedup {
    fn new(cap: usize) -> Self {
        Self { cap: cap.max(1), seen: HashMap::new(), order: VecDeque::new() }
    }

    fn contains(&self, external_id: &str, content_hash: &str) -> bool {
        self.seen.get(external_id).is_some_and(|hash| hash == content_hash)
    }

    fn insert(&mut self, external_id: String, content_hash: String) {
        if self.seen.insert(external_id.clone(), content_hash).is_none() {
            self.order.push_back(external_id);
            while self.order.len() > self.cap {
                if let Some(evicted) = self.order.pop_front() {
                    self.seen.remove(&evicted);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Dedup;

    #[test]
    fn dedup_skips_only_identical_id_and_hash() {
        let mut dedup = Dedup::new(8);
        assert!(!dedup.contains("a", "h1"));
        dedup.insert("a".to_owned(), "h1".to_owned());
        assert!(dedup.contains("a", "h1"));
        // Same id, changed content re-uploads.
        assert!(!dedup.contains("a", "h2"));
    }

    #[test]
    fn dedup_evicts_oldest_past_capacity() {
        let mut dedup = Dedup::new(2);
        dedup.insert("a".to_owned(), "h".to_owned());
        dedup.insert("b".to_owned(), "h".to_owned());
        dedup.insert("c".to_owned(), "h".to_owned());
        // "a" was evicted, "b"/"c" remain.
        assert!(!dedup.contains("a", "h"));
        assert!(dedup.contains("b", "h"));
        assert!(dedup.contains("c", "h"));
    }
}
