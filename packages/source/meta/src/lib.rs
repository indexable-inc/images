//! Shared metadata model for multi-source search.
//!
//! Every record uploaded to the store, whatever its source, carries a common
//! [`DocumentMeta`] header (flattened to top-level metadata keys so each is a
//! filter key) plus source-specific extras the adapter projects in. A
//! [`SourceAdapter`] turns one corpus (a code checkout, a Slack export, a Linear
//! export) into a stream of [`Document`]s ready to embed.
//!
//! Two invariants this crate enforces by construction:
//!
//! 1. **`content_hash` is the hash of the embedded bytes.** [`hash_body`] is the
//!    only way to make one, so a record's change-detection key can never drift
//!    from what was actually embedded. Re-ingesting an unchanged export is a
//!    no-op; a genuinely changed record re-embeds.
//! 2. **Metadata stays inside the store's limits.** [`check_metadata`] is a
//!    typed gate, so an over-budget record fails observably before upload rather
//!    than as an opaque 400 mid-bulk-ingest.
//!
//! This crate is pure data and traits: serde, a hash helper, and the trait. It
//! has no network or filesystem dependency, so both `search-core` and each
//! source adapter can depend on it without pulling in a client.

pub mod keys;

use std::fmt;
use std::future::Future;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use snafu::{ResultExt as _, Snafu, ensure};

/// Which corpus a record came from: an open string tag, not a closed set.
///
/// A source is a tag value stored under [`keys::SOURCE`] and used as the primary
/// scope filter, so adding a corpus (a Slack export, Claude Code history,
/// anything later) is a new tag value, never an enum variant or a new match arm
/// in the search pipeline. The newtype keeps it a typed boundary (callers pass a
/// `Source`, not a bare string), while [`Source::code`] and [`Source::web`] name
/// the two corpora the pipeline treats specially (local-manifest scoping and
/// opt-in web results); every other tag is a generic record source.
///
/// `#[serde(transparent)]` keeps the wire form a bare string, identical to the
/// values already stored (`"code"`, `"slack"`, ...), so existing records read
/// back unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Source(String);

impl Source {
    /// Wrap a tag value (e.g. `"slack"`, `"claude_history"`).
    #[must_use]
    pub fn new(tag: impl Into<String>) -> Self {
        Self(tag.into())
    }

    /// The `code` corpus: files in a git checkout, scoped against a local
    /// manifest by the search pipeline.
    #[must_use]
    pub fn code() -> Self {
        Self("code".to_owned())
    }

    /// The `web` corpus: hosted web-search results with no local record,
    /// included only when web results are requested.
    #[must_use]
    pub fn web() -> Self {
        Self("web".to_owned())
    }

    /// The tag string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this is the `code` corpus (manifest-scoped in the pipeline).
    #[must_use]
    pub fn is_code(&self) -> bool {
        self.0 == "code"
    }

    /// Whether this is the `web` corpus (included only on web queries).
    #[must_use]
    pub fn is_web(&self) -> bool {
        self.0 == "web"
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for Source {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<String> for Source {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl std::str::FromStr for Source {
    // Any string is a valid tag, so parsing never fails; the impl exists so
    // `s.parse::<Source>()` keeps working at call sites.
    type Err = std::convert::Infallible;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(Self(value.to_owned()))
    }
}

/// The repository a code record belongs to.
///
/// Used for cross-repo vs single-repo scoping. There is no silent empty-string
/// fallback: a checkout with no git remote is named by its directory, observably
/// [`RepoSlug::Local`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum RepoSlug {
    /// Derived from the git remote (e.g. `indexable-inc/index`).
    Remote(String),
    /// No remote; the checkout's directory name.
    Local(String),
}

impl RepoSlug {
    /// The slug string, regardless of origin.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Remote(slug) | Self::Local(slug) => slug,
        }
    }
}

impl fmt::Display for RepoSlug {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The common header on every stored record, flattened to top-level metadata.
///
/// Each field is a filter key. Significant fields are required (no
/// `serde(default)`); only genuinely optional ones are skipped when absent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentMeta {
    /// Which corpus this came from.
    pub source: Source,
    /// Stable per-record id; equals the store `external_id` (<= 256 chars).
    pub external_id: String,
    /// sha256 of the embedded body; drives skip-if-unchanged reconcile.
    pub content_hash: String,
    /// Human label for display (`rel_path`, `#craft: subject`, `ENG-1885: title`).
    pub title: String,
    /// Deep link back to the source, when one exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Primary recency axis, epoch seconds, when the record has a time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,
}

/// One record ready to upload: its identity, the bytes to embed, and the flat
/// metadata object (common header merged with source extras).
#[derive(Debug, Clone)]
pub struct Document {
    /// Stable per-record id; the store `external_id`.
    pub external_id: String,
    /// File name for the upload's `/v1/files` leg.
    pub file_name: String,
    /// MIME type the adapter chose for the body.
    pub mime: &'static str,
    /// The UTF-8 text that gets embedded.
    pub body: Vec<u8>,
    /// Flat metadata: [`DocumentMeta`] plus the adapter's source-specific extras.
    pub meta_json: serde_json::Value,
    /// sha256 of `body`; equals `meta_json["content_hash"]`.
    pub content_hash: String,
}

/// Turns one corpus into a stream of [`Document`]s.
///
/// `documents` returns an iterator, not a `Vec`, so a large export (a 344 MB
/// Slack tree) is streamed channel by channel. A record that cannot be parsed
/// is a typed `Err`, never silently dropped.
pub trait SourceAdapter {
    /// Adapter-specific failure type.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Which corpus this adapter ingests. Used to scope reconcile and GC to one
    /// source's records in the shared store.
    fn source(&self) -> Source;

    /// The current desired-state documents for this corpus.
    fn documents(&self) -> impl Iterator<Item = Result<Document, Self::Error>> + Send;
}

/// Converges one external view to a source's current desired-state document set.
///
/// The consumer counterpart of [`SourceAdapter`]: adapters produce desired
/// state, reconcilers make a view (a search index, an object-store log, an
/// analytics table) match it.
///
/// The contract every implementation upholds:
///
/// 1. **Desired state, not deltas.** `documents` is `source`'s complete current
///    set. Implementations converge toward it; what absence means (keep vs
///    delete) is each implementation's documented choice.
/// 2. **Idempotent.** Reconciling the same set twice is a no-op, keyed on
///    `external_id` + `content_hash` (see [`hash_body`]).
/// 3. **Source-scoped.** A reconcile reads and writes only `source`'s records,
///    never another source's.
///
/// A view can also satisfy this contract at the engine level instead of
/// implementing the trait: replayed duplicates collapsing in storage (e.g. a
/// `ClickHouse` `ReplacingMergeTree` whose sorting key is the record's natural
/// identity) is the same idempotence, declared rather than coded.
pub trait Reconciler {
    /// Per-pass outcome; each view reports its own shape.
    type Report;
    /// Reconcile failure type.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Converge the view to `documents`, the current desired state of `source`.
    fn reconcile(
        &self,
        source: &Source,
        documents: &[Document],
    ) -> impl Future<Output = Result<Self::Report, Self::Error>> + Send;
}

/// The `content_hash` of a body: `sha256:<hex>` over the exact bytes embedded.
///
/// This is the only constructor, so a record's change-detection hash is always
/// the hash of what was embedded. For code, the body is the file bytes, so this
/// equals the existing content-addressed id.
#[must_use]
pub fn hash_body(body: &[u8]) -> String {
    let digest = Sha256::digest(body);
    let mut out = String::with_capacity("sha256:".len() + digest.len() * 2);
    out.push_str("sha256:");
    for byte in digest {
        use fmt::Write as _;
        // Writing hex to a String is infallible; no panic path.
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Maximum serialized size of one record's metadata object, in bytes.
pub const MAX_METADATA_BYTES: usize = 128 * 1024;
/// Maximum number of top-level metadata keys on one record.
pub const MAX_METADATA_KEYS: usize = 256;

/// A record's metadata exceeded a store limit.
#[derive(Debug, Snafu)]
#[non_exhaustive]
pub enum MetadataError {
    /// Serialized metadata is larger than [`MAX_METADATA_BYTES`].
    #[snafu(display("metadata for {external_id} is {bytes} bytes, over the metadata size limit"))]
    TooLarge {
        /// The record whose metadata overflowed.
        external_id: String,
        /// Serialized byte length.
        bytes: usize,
    },
    /// Metadata has more than [`MAX_METADATA_KEYS`] top-level keys.
    #[snafu(display("metadata for {external_id} has {count} keys, over the metadata key limit"))]
    TooManyKeys {
        /// The record whose metadata overflowed.
        external_id: String,
        /// Top-level key count.
        count: usize,
    },
    /// Metadata serialization failed.
    #[snafu(display("failed to serialize metadata for {external_id}: {source}"))]
    Encode {
        /// The record whose metadata could not be encoded.
        external_id: String,
        /// Underlying serde error.
        source: serde_json::Error,
    },
}

/// Verify a record's flat metadata fits the store's limits before upload.
///
/// Returns the serialized bytes on success so the caller does not serialize
/// twice. Fails observably rather than letting an over-budget record become an
/// opaque 400 in the middle of a bulk ingest.
///
/// # Errors
/// Returns [`MetadataError`] if the metadata is too large, has too many keys, or
/// cannot be serialized.
pub fn check_metadata(
    external_id: &str,
    meta: &serde_json::Value,
) -> Result<Vec<u8>, MetadataError> {
    if let Some(object) = meta.as_object() {
        ensure!(
            object.len() <= MAX_METADATA_KEYS,
            TooManyKeysSnafu {
                external_id,
                count: object.len(),
            }
        );
    }
    let bytes = serde_json::to_vec(meta).context(EncodeSnafu { external_id })?;
    ensure!(
        bytes.len() <= MAX_METADATA_BYTES,
        TooLargeSnafu {
            external_id,
            bytes: bytes.len(),
        }
    );
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::{Source, check_metadata, hash_body};

    #[test]
    fn source_round_trips_through_str() {
        for tag in [
            "code",
            "slack",
            "linear",
            "claude_history",
            "web",
            "anything-new",
        ] {
            let source = Source::new(tag);
            let parsed: Source = source.as_str().parse().expect("infallible");
            assert_eq!(parsed, source);
            assert_eq!(source.as_str(), tag);
        }
    }

    #[test]
    fn well_known_sources_classify() {
        assert!(Source::code().is_code());
        assert!(Source::web().is_web());
        assert!(!Source::new("slack").is_code());
        assert!(!Source::new("claude_history").is_web());
    }

    #[test]
    fn source_serializes_as_bare_string() {
        assert_eq!(
            serde_json::to_string(&Source::new("linear")).expect("ser"),
            "\"linear\""
        );
        let parsed: Source = serde_json::from_str("\"slack\"").expect("de");
        assert_eq!(parsed, Source::new("slack"));
    }

    #[test]
    fn hash_is_stable_and_prefixed() {
        let a = hash_body(b"hello world");
        let b = hash_body(b"hello world");
        assert_eq!(a, b);
        assert!(a.starts_with("sha256:"));
        assert_ne!(a, hash_body(b"hello worlds"));
    }

    #[test]
    fn metadata_within_limits_passes() {
        let meta = serde_json::json!({ "source": "code", "path": "a.rs" });
        assert!(check_metadata("sha256:x", &meta).is_ok());
    }

    #[test]
    fn oversized_metadata_is_rejected() {
        let big = "x".repeat(super::MAX_METADATA_BYTES + 1);
        let meta = serde_json::json!({ "source": "slack", "blob": big });
        assert!(check_metadata("slack:c:1", &meta).is_err());
    }
}
