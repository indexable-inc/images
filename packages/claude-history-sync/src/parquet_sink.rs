//! Write Claude transcript messages to an S3-compatible bucket (Cloudflare R2)
//! as hive-partitioned parquet, one file per session, so polars and duckdb can
//! query the whole tree (`scan_parquet("s3://.../claude-history/**/*.parquet")`).
//!
//! A per-host manifest object (`<prefix>/_manifest/<host>.json`) records each
//! session's content hash, so a re-run only re-uploads sessions whose
//! transcript actually changed. The layout is
//! `<prefix>/host=<h>/user=<u>/project=<p>/session=<sid>.parquet`; partition
//! values are sanitized so a `cwd`-derived project never injects path
//! separators.

use std::collections::BTreeMap;
use std::sync::Arc;

use arrow::array::{ArrayRef, Int64Array, RecordBatch, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use claude_history::Message;
use object_store::aws::{AmazonS3, AmazonS3Builder};
use object_store::path::Path as ObjectPath;
use object_store::{ObjectStoreExt, PutPayload};
use parquet::arrow::ArrowWriter;
use sha2::{Digest as _, Sha256};
use snafu::{IntoError as _, ResultExt as _, Snafu};

/// Connection and layout for the S3/R2 archive sink.
#[derive(Debug, Clone)]
pub struct Config {
    /// Target bucket name.
    pub bucket: String,
    /// S3 endpoint URL. `None` uses AWS S3; for R2 pass the account endpoint.
    pub endpoint: Option<String>,
    /// Region (`auto` for R2).
    pub region: String,
    /// Key prefix under the bucket (e.g. `claude-history`).
    pub prefix: String,
    /// Short hostname, the manifest key and the `host=` partition value.
    pub host: String,
}

/// Failures from the S3 parquet sink.
#[derive(Debug, Snafu)]
#[non_exhaustive]
pub enum Error {
    /// The S3 client could not be built from the config and environment.
    #[snafu(display("failed to build the S3 client for bucket {bucket}"))]
    BuildStore {
        /// Bucket the client targeted.
        bucket: String,
        /// Underlying object-store error.
        source: object_store::Error,
    },
    /// An object could not be read.
    #[snafu(display("failed to read object {path}"))]
    Get {
        /// Object key.
        path: String,
        /// Underlying object-store error.
        source: object_store::Error,
    },
    /// An object could not be written.
    #[snafu(display("failed to write object {path}"))]
    Put {
        /// Object key.
        path: String,
        /// Underlying object-store error.
        source: object_store::Error,
    },
    /// The manifest object did not parse as JSON.
    #[snafu(display("failed to parse the manifest {path}"))]
    Manifest {
        /// Manifest key.
        path: String,
        /// Underlying serde error.
        source: serde_json::Error,
    },
    /// The manifest could not be serialized.
    #[snafu(display("failed to serialize the manifest"))]
    SerializeManifest {
        /// Underlying serde error.
        source: serde_json::Error,
    },
    /// A record batch could not be assembled from a session.
    #[snafu(display("failed to build the record batch"))]
    Batch {
        /// Underlying arrow error.
        source: arrow::error::ArrowError,
    },
    /// Parquet encoding failed.
    #[snafu(display("failed to encode parquet"))]
    Encode {
        /// Underlying parquet error.
        source: parquet::errors::ParquetError,
    },
}

/// Result alias defaulting to this module's [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Outcome of one S3 sync pass.
#[derive(Debug, Clone, Copy)]
pub struct Report {
    /// Sessions whose parquet was (re)uploaded.
    pub uploaded: usize,
    /// Sessions skipped because their content hash was unchanged.
    pub skipped: usize,
}

/// Sync `messages` to the bucket as per-session parquet, skipping sessions whose
/// content hash matches the per-host manifest.
///
/// # Errors
/// Returns an error if the client cannot be built, the manifest or any object
/// cannot be read or written, or a batch cannot be encoded.
pub async fn sync(messages: &[Message], config: &Config) -> Result<Report> {
    let store = build_store(config)?;
    let manifest_path =
        ObjectPath::from(format!("{}/_manifest/{}.json", config.prefix, sanitize(&config.host)));
    let mut manifest = load_manifest(&store, &manifest_path).await?;

    // Group by session, preserving transcript order within each session so the
    // content hash is stable for an unchanged session.
    let mut by_session: BTreeMap<&str, Vec<&Message>> = BTreeMap::new();
    for message in messages {
        by_session.entry(message.session_id.as_str()).or_default().push(message);
    }

    let mut uploaded = 0;
    let mut skipped = 0;
    let mut failure = None;
    for (session_id, session) in &by_session {
        let Some(first) = session.first().copied() else {
            continue;
        };
        let hash = session_hash(session);
        if manifest.get(*session_id).is_some_and(|existing| existing == &hash) {
            skipped += 1;
            continue;
        }

        let batch = record_batch(session)?;
        let bytes = encode_parquet(&batch)?;
        let key = object_key(config, first, session_id);
        // Record each success in the manifest as it lands, and persist the
        // manifest even if a later put fails, so a re-run does not re-upload the
        // sessions that already succeeded this pass.
        match store.put(&key, PutPayload::from(bytes)).await {
            Ok(_) => {
                manifest.insert((*session_id).to_owned(), hash);
                uploaded += 1;
            }
            Err(source) => {
                failure = Some(PutSnafu { path: key.to_string() }.into_error(source));
                break;
            }
        }
    }

    save_manifest(&store, &manifest_path, &manifest).await?;
    if let Some(failure) = failure {
        return Err(failure);
    }
    Ok(Report { uploaded, skipped })
}

/// Build the S3 client. Credentials come from the environment
/// (`AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`).
fn build_store(config: &Config) -> Result<AmazonS3> {
    let mut builder = AmazonS3Builder::from_env()
        .with_bucket_name(&config.bucket)
        .with_region(&config.region);
    if let Some(endpoint) = &config.endpoint {
        builder = builder.with_endpoint(endpoint);
    }
    builder
        .build()
        .context(BuildStoreSnafu { bucket: config.bucket.clone() })
}

/// Object key for one session's parquet, hive-partitioned by host/user/project.
fn object_key(config: &Config, first: &Message, session_id: &str) -> ObjectPath {
    ObjectPath::from(format!(
        "{}/host={}/user={}/project={}/session={}.parquet",
        config.prefix,
        sanitize(&first.host),
        sanitize(&first.user),
        sanitize(&first.project),
        sanitize(session_id),
    ))
}

/// Load the per-host manifest, or an empty map when it does not exist yet.
async fn load_manifest(store: &AmazonS3, path: &ObjectPath) -> Result<BTreeMap<String, String>> {
    let result = match store.get(path).await {
        Ok(result) => result,
        Err(object_store::Error::NotFound { .. }) => return Ok(BTreeMap::new()),
        Err(source) => return Err(GetSnafu { path: path.to_string() }.into_error(source)),
    };
    let bytes = result.bytes().await.context(GetSnafu { path: path.to_string() })?;
    serde_json::from_slice(&bytes).context(ManifestSnafu { path: path.to_string() })
}

/// Write the manifest back.
async fn save_manifest(
    store: &AmazonS3,
    path: &ObjectPath,
    manifest: &BTreeMap<String, String>,
) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(manifest).context(SerializeManifestSnafu)?;
    store
        .put(path, PutPayload::from(bytes))
        .await
        .context(PutSnafu { path: path.to_string() })?;
    Ok(())
}

/// The arrow schema: the per-message fields as nullable columns.
///
/// `host`/`user`/`project`/`session` are intentionally NOT columns: they are the
/// hive partition keys in the object path. Polars `hive_partitioning=True` would
/// otherwise drop a same-named file column and substitute the (sanitized) path
/// value, which for `project` loses the real `/`-bearing path. The authoritative
/// per-message working directory is kept in `cwd` (not a partition key, so it
/// survives), and `session_id` is kept too (the `session=` key uses a different
/// name, so there is no clobber).
fn schema() -> Schema {
    let text = |name: &str| Field::new(name, DataType::Utf8, true);
    let int = |name: &str| Field::new(name, DataType::Int64, true);
    Schema::new(vec![
        text("session_id"),
        text("message_uuid"),
        text("parent_uuid"),
        text("role"),
        text("record_type"),
        text("model"),
        text("cwd"),
        text("git_branch"),
        text("tool_name"),
        int("input_tokens"),
        int("output_tokens"),
        // Epoch seconds (matches the Mixedbread `timestamp` tag).
        int("timestamp"),
        text("body"),
    ])
}

/// Build one session's record batch (one row per message).
fn record_batch(session: &[&Message]) -> Result<RecordBatch> {
    let columns: Vec<ArrayRef> = vec![
        text_column(session, |m| Some(m.session_id.as_str())),
        text_column(session, |m| Some(m.uuid.as_str())),
        text_column(session, |m| m.parent_uuid.as_deref()),
        text_column(session, |m| Some(m.role.as_str())),
        text_column(session, |m| Some(m.record_type.as_str())),
        text_column(session, |m| m.model.as_deref()),
        text_column(session, |m| m.cwd.as_deref()),
        text_column(session, |m| m.git_branch.as_deref()),
        text_column(session, |m| m.tool_name.as_deref()),
        int_column(session, |m| m.input_tokens),
        int_column(session, |m| m.output_tokens),
        int_column(session, |m| m.timestamp),
        text_column(session, |m| Some(m.body.as_str())),
    ];
    RecordBatch::try_new(Arc::new(schema()), columns).context(BatchSnafu)
}

/// A nullable UTF-8 column projected from each message.
fn text_column(session: &[&Message], project: impl Fn(&Message) -> Option<&str>) -> ArrayRef {
    Arc::new(session.iter().copied().map(project).collect::<StringArray>())
}

/// A nullable i64 column projected from each message.
fn int_column(session: &[&Message], project: impl Fn(&Message) -> Option<i64>) -> ArrayRef {
    Arc::new(session.iter().copied().map(project).collect::<Int64Array>())
}

/// Encode a record batch to parquet bytes in memory.
fn encode_parquet(batch: &RecordBatch) -> Result<Vec<u8>> {
    let mut buffer = Vec::new();
    let mut writer = ArrowWriter::try_new(&mut buffer, batch.schema(), None).context(EncodeSnafu)?;
    writer.write(batch).context(EncodeSnafu)?;
    writer.close().context(EncodeSnafu)?;
    Ok(buffer)
}

/// Content hash of a session: sha256 over each message's uuid and body, in
/// transcript order. Appending a message changes the hash, triggering a
/// re-upload; an unchanged session hashes identically and is skipped.
fn session_hash(session: &[&Message]) -> String {
    let mut digest = Sha256::new();
    for message in session.iter().copied() {
        digest.update(message.uuid.as_bytes());
        digest.update([0]);
        digest.update(message.body.as_bytes());
        digest.update([0]);
    }
    format!("{:x}", digest.finalize())
}

/// Make a partition value safe for an object key: keep alphanumerics and
/// `.`/`_`/`-`, replace everything else (notably `/` from a `cwd` path) with `_`.
fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{Config, object_key, record_batch, sanitize, schema, session_hash};
    use claude_history::Message;

    fn sample(uuid: &str, body: &str) -> Message {
        Message {
            host: "host1".to_owned(),
            user: "user1".to_owned(),
            project: "/Users/x/proj".to_owned(),
            session_id: "sess1".to_owned(),
            uuid: uuid.to_owned(),
            parent_uuid: None,
            role: "user".to_owned(),
            record_type: "user".to_owned(),
            model: None,
            cwd: Some("/Users/x/proj".to_owned()),
            git_branch: None,
            tool_name: None,
            input_tokens: None,
            output_tokens: None,
            timestamp: Some(1),
            body: body.to_owned(),
        }
    }

    #[test]
    fn sanitize_replaces_path_unsafe_chars() {
        assert_eq!(sanitize("/Users/x/a-b.c"), "_Users_x_a-b.c");
        assert_eq!(sanitize("plain"), "plain");
        assert_eq!(sanitize("a b/c=d"), "a_b_c_d");
    }

    #[test]
    fn session_hash_is_stable_and_body_sensitive() {
        let first = sample("u1", "hello");
        let same = sample("u1", "hello");
        let changed = sample("u1", "changed");
        assert_eq!(session_hash(&[&first]), session_hash(&[&same]));
        assert_ne!(session_hash(&[&first]), session_hash(&[&changed]));
    }

    #[test]
    fn distinct_messages_do_not_collide() {
        let a = sample("u1", "x");
        let b = sample("u2", "y");
        assert_ne!(session_hash(&[&a]), session_hash(&[&b]));
    }

    #[test]
    fn object_key_is_hive_partitioned_and_sanitized() {
        let config = Config {
            bucket: "b".to_owned(),
            endpoint: None,
            region: "auto".to_owned(),
            prefix: "claude-history".to_owned(),
            host: "host1".to_owned(),
        };
        let key = object_key(&config, &sample("u1", "x"), "sess1");
        assert_eq!(
            key.to_string(),
            "claude-history/host=host1/user=user1/project=_Users_x_proj/session=sess1.parquet"
        );
    }

    #[test]
    fn record_batch_columns_match_schema() {
        let a = sample("u1", "x");
        let b = sample("u2", "y");
        let batch = record_batch(&[&a, &b]).expect("batch");
        assert_eq!(batch.num_columns(), schema().fields().len());
        assert_eq!(batch.num_rows(), 2);
    }
}
