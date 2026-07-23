//! The lake table's schema and the row ↔ [`Document`] codec.
//!
//! One row is one *observation*: a source's document as seen by one writer
//! slice (`host`, optional `user`), tagged `op = upsert`, or a tombstone
//! (`op = delete`) recording that the slice's writer EXPLICITLY deleted it —
//! a gc pass over an export-complete source, never mere absence from a newer
//! pass (ENG-2696). The table is an append-only revision log; current state
//! is a per-slice fold ordered by `version` (see [`fold_slices`]): an id is
//! live while any slice's latest op for it is an upsert.
//!
//! `version` is the slice's revision counter, assigned by the writer as its
//! slice's previous maximum plus one. It is committed data, so it survives
//! compaction and clock steps; `observed_at` (wall-clock epoch ms) is kept for
//! queryability and as the freshness arbiter between live replicas of one id
//! in different slices, never to order one slice's operations.
//!
//! The first nine columns are exactly `sink-parquet`'s flat corpus schema, so
//! every existing polars/duckdb query ports by adding `op != 'delete'` to its
//! filter. `user`, `op`, `observed_at`, and `version` are the log's additions.
//! As in the parquet log, `source`/`title`/`url`/`host`/`timestamp`/`user` are
//! projections for queryability: a [`Document`] is reconstructed from
//! `external_id`, `content_hash`, `body`, and `meta_json` alone.
//!
//! Nullability rule: `content_hash`, `body`, and `meta_json` are null exactly
//! when `op = delete` (a tombstone carries identity, not content). A null in
//! any of them on an upsert row is a malformed log and decodes to a typed
//! error, never a default.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Arc;

use arrow_array::{Array as _, ArrayRef, Int64Array, RecordBatch, StringArray};
use arrow_schema::Schema as ArrowSchema;
use iceberg::spec::{NestedField, PrimitiveType, Schema, Type};
use snafu::{OptionExt as _, ResultExt as _};
use source_meta::{Document, Source, keys};

use crate::error::{
    BadOpSnafu, BatchSnafu, ColumnTypeSnafu, MetaJsonSnafu, MissingColumnSnafu, NullValueSnafu,
    Result, SchemaSnafu, TombstoneDocumentSnafu,
};

/// `op` value for a document observation.
pub const OP_UPSERT: &str = "upsert";
/// `op` value for a tombstone (an explicit gc deletion; absence from a newer
/// pass never tombstones — ENG-2696).
pub const OP_DELETE: &str = "delete";

/// The non-payload columns every read needs: identity, grouping (`source`),
/// the slice and version fold keys, the change-detection hash, and the
/// cross-slice freshness arbiter. Enough to diff a slice's live state against
/// desired state or to judge global liveness, without hauling bodies.
pub const STATE_COLUMNS: [&str; 8] = [
    "external_id",
    "source",
    "content_hash",
    "host",
    "user",
    "op",
    "observed_at",
    "version",
];

/// [`STATE_COLUMNS`] plus the payload a [`Document`] is reconstructed from
/// (`body`, `meta_json`); the rest of the schema is a projection out of
/// `meta_json`, mirroring `source-parquet`'s four-column rule.
pub const CODEC_COLUMNS: [&str; 10] = [
    "external_id",
    "source",
    "content_hash",
    "host",
    "user",
    "op",
    "observed_at",
    "version",
    "body",
    "meta_json",
];

/// The lake's Iceberg schema. Field ids are stable and append-only: the first
/// nine are `sink-parquet`'s columns in its order, then the log's additions.
pub fn table_schema() -> Result<Schema> {
    let optional = |id: i32, name: &str| {
        NestedField::optional(id, name, Type::Primitive(PrimitiveType::String))
    };
    let required = |id: i32, name: &str| {
        NestedField::required(id, name, Type::Primitive(PrimitiveType::String))
    };
    Schema::builder()
        .with_schema_id(0)
        .with_fields(vec![
            required(1, "external_id").into(),
            required(2, "source").into(),
            // Null only on op=delete rows (a tombstone carries no content).
            optional(3, "content_hash").into(),
            optional(4, "title").into(),
            optional(5, "url").into(),
            required(6, "host").into(),
            NestedField::optional(7, "timestamp", Type::Primitive(PrimitiveType::Long)).into(),
            optional(8, "body").into(),
            optional(9, "meta_json").into(),
            optional(10, "user").into(),
            required(11, "op").into(),
            NestedField::required(12, "observed_at", Type::Primitive(PrimitiveType::Long)).into(),
            NestedField::required(13, "version", Type::Primitive(PrimitiveType::Long)).into(),
        ])
        .build()
        .context(SchemaSnafu)
}

/// One writer's slice of the corpus: which host (and account, for the per-user
/// fleet path) the observations belong to. Tombstones are scoped to a slice so
/// host A never deletes what host B still observes.
#[derive(Debug, Clone, Copy)]
pub struct Slice<'a> {
    /// The writing host (`networking.hostName` on the fleet).
    pub host: &'a str,
    /// The account, for per-user sources; `None` for host-level bulk exports.
    pub user: Option<&'a str>,
}

/// Encode one write pass — a reconcile's upserts or a gc's tombstones — as a
/// single record batch against the table's arrow schema (which carries the
/// parquet field-id metadata the writer requires). `version` is the slice's
/// revision counter for this pass (its previous maximum plus one).
pub fn encode_batch(
    arrow_schema: &Arc<ArrowSchema>,
    source: &Source,
    slice: Slice<'_>,
    observed_at: i64,
    version: i64,
    upserts: &[&Document],
    deletes: &[&str],
) -> Result<RecordBatch> {
    let n = upserts.len() + deletes.len();
    let up = upserts.len();
    let meta_str = |doc: &Document, key: &str| {
        doc.meta_json
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    };
    // Upsert rows first, tombstones after; per-column closures keep each
    // column a single pass over both halves.
    let string_col = |f: &dyn Fn(usize) -> Option<String>| -> ArrayRef {
        Arc::new((0..n).map(f).collect::<StringArray>())
    };
    let columns: Vec<ArrayRef> = vec![
        string_col(&|i| {
            Some(if i < up {
                upserts[i].external_id.clone()
            } else {
                deletes[i - up].to_owned()
            })
        }),
        string_col(&|_| Some(source.as_str().to_owned())),
        string_col(&|i| (i < up).then(|| upserts[i].content_hash.clone())),
        string_col(&|i| {
            (i < up)
                .then(|| meta_str(upserts[i], keys::TITLE))
                .flatten()
        }),
        string_col(&|i| (i < up).then(|| meta_str(upserts[i], "url")).flatten()),
        string_col(&|_| Some(slice.host.to_owned())),
        Arc::new(
            (0..n)
                .map(|i| {
                    (i < up)
                        .then(|| {
                            upserts[i]
                                .meta_json
                                .get(keys::TIMESTAMP)
                                .and_then(serde_json::Value::as_i64)
                        })
                        .flatten()
                })
                .collect::<Int64Array>(),
        ),
        string_col(&|i| (i < up).then(|| String::from_utf8_lossy(&upserts[i].body).into_owned())),
        string_col(&|i| (i < up).then(|| upserts[i].meta_json.to_string())),
        string_col(&|_| slice.user.map(str::to_owned)),
        string_col(&|i| Some((if i < up { OP_UPSERT } else { OP_DELETE }).to_owned())),
        Arc::new((0..n).map(|_| Some(observed_at)).collect::<Int64Array>()),
        Arc::new((0..n).map(|_| Some(version)).collect::<Int64Array>()),
    ];
    RecordBatch::try_new(Arc::clone(arrow_schema), columns).context(BatchSnafu)
}

/// A row's operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// The document was observed in the writer's desired state.
    Upsert,
    /// The writer explicitly deleted the document (a gc pass over an
    /// export-complete source; never inferred from absence — ENG-2696).
    Delete,
}

/// One decoded log row: identity, fold keys, and (for payload reads) the
/// content needed to reconstruct a [`Document`].
#[derive(Debug)]
pub struct LakeRow {
    /// Stable per-record id (the store `external_id`).
    pub external_id: String,
    /// The source the row belongs to (grouping key for the per-source view).
    pub source: String,
    /// The writing host (slice fold key).
    pub host: String,
    /// The account, for per-user slices (slice fold key).
    pub user: Option<String>,
    /// sha256 of the body; `None` only on tombstones.
    pub content_hash: Option<String>,
    /// The embedded text; `None` on tombstones, or when the read was projected
    /// to [`STATE_COLUMNS`].
    pub body: Option<String>,
    /// The full metadata object as JSON; `None` like `body`.
    pub meta_json: Option<String>,
    /// The row's operation.
    pub op: Op,
    /// When the writer observed this state (epoch milliseconds). Informational
    /// and a cross-slice freshness arbiter; never orders a slice's operations.
    pub observed_at: i64,
    /// The slice's committed revision counter; orders the slice's operations.
    pub version: i64,
}

/// Decode one record batch into rows, appending to `out`. `with_payload` says
/// whether `body`/`meta_json` are expected in the batch (a state-projected
/// scan omits them).
pub fn rows_from_batch(
    batch: &RecordBatch,
    with_payload: bool,
    out: &mut Vec<LakeRow>,
) -> Result<()> {
    let external_id = string_column(batch, "external_id")?;
    let source = string_column(batch, "source")?;
    let host = string_column(batch, "host")?;
    let user = string_column(batch, "user")?;
    let content_hash = string_column(batch, "content_hash")?;
    let op_col = string_column(batch, "op")?;
    let observed_at = long_column(batch, "observed_at")?;
    let version = long_column(batch, "version")?;
    let payload = if with_payload {
        Some((
            string_column(batch, "body")?,
            string_column(batch, "meta_json")?,
        ))
    } else {
        None
    };

    out.reserve(batch.num_rows());
    for row in 0..batch.num_rows() {
        let op = match non_null_str(op_col, row, "op")? {
            OP_UPSERT => Op::Upsert,
            OP_DELETE => Op::Delete,
            other => {
                return BadOpSnafu {
                    value: other.to_owned(),
                    row,
                }
                .fail();
            }
        };
        let opt = |array: &StringArray| array.is_valid(row).then(|| array.value(row).to_owned());
        let (body, meta_json) = match &payload {
            Some((body, meta_json)) => (opt(body), opt(meta_json)),
            None => (None, None),
        };
        // An upsert row missing its content is a malformed log, surfaced as a
        // typed error rather than reconstructed from defaults.
        if op == Op::Upsert {
            if content_hash.is_null(row) {
                return NullValueSnafu {
                    column: "content_hash",
                    row,
                }
                .fail();
            }
            if with_payload && body.is_none() {
                return NullValueSnafu {
                    column: "body",
                    row,
                }
                .fail();
            }
            if with_payload && meta_json.is_none() {
                return NullValueSnafu {
                    column: "meta_json",
                    row,
                }
                .fail();
            }
        }
        let long = |array: &Int64Array, column: &'static str| {
            array
                .is_valid(row)
                .then(|| array.value(row))
                .context(NullValueSnafu { column, row })
        };
        out.push(LakeRow {
            external_id: non_null_str(external_id, row, "external_id")?.to_owned(),
            source: non_null_str(source, row, "source")?.to_owned(),
            host: non_null_str(host, row, "host")?.to_owned(),
            user: opt(user),
            content_hash: opt(content_hash),
            body,
            meta_json,
            op,
            observed_at: long(observed_at, "observed_at")?,
            version: long(version, "version")?,
        });
    }
    Ok(())
}

/// Reconstruct a [`Document`] from an upsert row, mirroring `source-parquet`'s
/// conventions exactly: `file_name` is the `external_id`, the mime is plain
/// text, and `meta_json` is parsed whole (source extras intact).
///
/// Calling this on a tombstone or state-projected row is a typed error
/// ([`rows_from_batch`] already validated payload presence for upserts).
pub fn document_from_row(row: LakeRow) -> Result<Document> {
    let tombstone = |what: &'static str| TombstoneDocumentSnafu { what };
    let body = row.body.context(tombstone("body"))?.into_bytes();
    let meta_str = row.meta_json.context(tombstone("meta_json"))?;
    let meta = serde_json::from_str(&meta_str).context(MetaJsonSnafu)?;
    Ok(Document {
        file_name: row.external_id.clone(),
        external_id: row.external_id,
        mime: "text/plain",
        body,
        meta_json: meta,
        content_hash: row.content_hash.context(tombstone("content_hash"))?,
    })
}

/// Fold rows into current state: per `external_id`, each slice's latest row.
///
/// Within a slice, "latest" is the greatest `version` — the writer's committed
/// revision counter — so ordering is immune to wall-clock steps and to
/// compaction rewriting files. (A version tie means two writers raced one
/// slice, which the fleet's serialized oneshot units rule out; it breaks by
/// `observed_at`, then the later-read row, to stay deterministic.) Slices are
/// kept apart because tombstones are slice-scoped: one host's delete must not
/// erase a record another slice still observes — liveness belongs to
/// [`live_winner`].
pub fn fold_slices(rows: Vec<LakeRow>) -> HashMap<String, Vec<LakeRow>> {
    let mut latest: HashMap<String, HashMap<(String, Option<String>), LakeRow>> = HashMap::new();
    for row in rows {
        let slices = latest.entry(row.external_id.clone()).or_default();
        match slices.entry((row.host.clone(), row.user.clone())) {
            Entry::Occupied(mut entry) => {
                let held = entry.get();
                if (row.version, row.observed_at) >= (held.version, held.observed_at) {
                    entry.insert(row);
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(row);
            }
        }
    }
    latest
        .into_iter()
        .map(|(id, slices)| (id, slices.into_values().collect()))
        .collect()
}

/// The row representing one id's live state, given its slice-latest rows from
/// [`fold_slices`]: the id is live while *any* slice's latest op is an upsert,
/// and the greatest `observed_at` (ties: slice identity) among those upserts
/// picks which replica's content the view shows. Versions order operations
/// within a slice and are not comparable across slices, so wall clock here
/// only arbitrates between concurrently live copies of the same record — it
/// never decides whether an id lives or dies. `None` means every slice that
/// ever held the id has tombstoned it.
pub fn live_winner(rows: Vec<LakeRow>) -> Option<LakeRow> {
    rows.into_iter()
        .filter(|row| row.op == Op::Upsert)
        .max_by(|a, b| (a.observed_at, &a.host, &a.user).cmp(&(b.observed_at, &b.host, &b.user)))
}

/// Borrow one column as a `StringArray`, erroring (never defaulting) when the
/// column is absent or mis-typed.
fn string_column<'a>(batch: &'a RecordBatch, column: &'static str) -> Result<&'a StringArray> {
    let array = batch
        .column_by_name(column)
        .context(MissingColumnSnafu { column })?;
    array
        .as_any()
        .downcast_ref::<StringArray>()
        .context(ColumnTypeSnafu { column })
}

/// Borrow one column as an `Int64Array`, erroring like [`string_column`].
fn long_column<'a>(batch: &'a RecordBatch, column: &'static str) -> Result<&'a Int64Array> {
    let array = batch
        .column_by_name(column)
        .context(MissingColumnSnafu { column })?;
    array
        .as_any()
        .downcast_ref::<Int64Array>()
        .context(ColumnTypeSnafu { column })
}

/// Read one row of a required string column, erroring on a null cell.
fn non_null_str<'a>(array: &'a StringArray, row: usize, column: &'static str) -> Result<&'a str> {
    array
        .is_valid(row)
        .then(|| array.value(row))
        .context(NullValueSnafu { column, row })
}
