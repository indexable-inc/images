//! The lake table's schema and the row ↔ [`Document`] codec.
//!
//! One row is one *observation*: a source's document as seen by one writer
//! (`host`, optional `user`) at `observed_at`, tagged `op = upsert`, or a
//! tombstone (`op = delete`) recording that the document left that writer's
//! desired state. The table is an append-only revision log; current state is a
//! latest-wins fold per `external_id` (see [`crate::read_all`]).
//!
//! The first nine columns are exactly `sink-parquet`'s flat corpus schema, so
//! every existing polars/duckdb query ports by adding `op != 'delete'` to its
//! filter. `user`, `op`, and `observed_at` are the log's additions. As in the
//! parquet log, `source`/`title`/`url`/`host`/`timestamp`/`user` are
//! projections for queryability: a [`Document`] is reconstructed from
//! `external_id`, `content_hash`, `body`, and `meta_json` alone.
//!
//! Nullability rule: `content_hash`, `body`, and `meta_json` are null exactly
//! when `op = delete` (a tombstone carries identity, not content). A null in
//! any of them on an upsert row is a malformed log and decodes to a typed
//! error, never a default.

use std::collections::HashMap;
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
/// `op` value for a tombstone (the document left the writer's desired state).
pub const OP_DELETE: &str = "delete";

/// The columns a [`Document`] (or tombstone) is reconstructed from; the rest of
/// the schema is a projection out of `meta_json`. Mirrors `source-parquet`'s
/// four-column rule, plus the log's `op` and `observed_at`.
pub(crate) const CODEC_COLUMNS: [&str; 6] =
    ["external_id", "content_hash", "body", "meta_json", "op", "observed_at"];

/// The columns the sink needs to diff a slice's live state against desired
/// state: identity, change-detection hash, and the fold keys.
pub(crate) const STATE_COLUMNS: [&str; 4] = ["external_id", "content_hash", "op", "observed_at"];

/// The lake's Iceberg schema. Field ids are stable and append-only: the first
/// nine are `sink-parquet`'s columns in its order, then the log's additions.
pub(crate) fn table_schema() -> Result<Schema> {
    let optional =
        |id: i32, name: &str| NestedField::optional(id, name, Type::Primitive(PrimitiveType::String));
    let required =
        |id: i32, name: &str| NestedField::required(id, name, Type::Primitive(PrimitiveType::String));
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
        ])
        .build()
        .context(SchemaSnafu)
}

/// One writer's slice of the corpus: which host (and account, for the per-user
/// fleet path) the observations belong to. Tombstones are scoped to a slice so
/// host A never deletes what host B still observes.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Slice<'a> {
    /// The writing host (`networking.hostName` on the fleet).
    pub host: &'a str,
    /// The account, for per-user sources; `None` for host-level bulk exports.
    pub user: Option<&'a str>,
}

/// Encode one reconcile pass — upserts plus tombstones — as a single record
/// batch against the table's arrow schema (which carries the parquet field-id
/// metadata the writer requires).
pub(crate) fn encode_batch(
    arrow_schema: &Arc<ArrowSchema>,
    source: &Source,
    slice: Slice<'_>,
    observed_at: i64,
    upserts: &[&Document],
    deletes: &[&str],
) -> Result<RecordBatch> {
    let n = upserts.len() + deletes.len();
    let up = upserts.len();
    let meta_str = |doc: &Document, key: &str| {
        doc.meta_json.get(key).and_then(serde_json::Value::as_str).map(str::to_owned)
    };
    // Upsert rows first, tombstones after; per-column closures keep each
    // column a single pass over both halves.
    let string_col = |f: &dyn Fn(usize) -> Option<String>| -> ArrayRef {
        Arc::new((0..n).map(f).collect::<StringArray>())
    };
    let columns: Vec<ArrayRef> = vec![
        string_col(&|i| {
            Some(if i < up { upserts[i].external_id.clone() } else { deletes[i - up].to_owned() })
        }),
        string_col(&|_| Some(source.as_str().to_owned())),
        string_col(&|i| (i < up).then(|| upserts[i].content_hash.clone())),
        string_col(&|i| (i < up).then(|| meta_str(upserts[i], keys::TITLE)).flatten()),
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
    ];
    RecordBatch::try_new(Arc::clone(arrow_schema), columns).context(BatchSnafu)
}

/// A row's operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    /// The document was observed in the writer's desired state.
    Upsert,
    /// The document left the writer's desired state.
    Delete,
}

/// One decoded log row: identity, fold keys, and (for payload reads) the
/// content needed to reconstruct a [`Document`].
#[derive(Debug)]
pub(crate) struct LakeRow {
    /// Stable per-record id (the store `external_id`).
    pub external_id: String,
    /// sha256 of the body; `None` only on tombstones.
    pub content_hash: Option<String>,
    /// The embedded text; `None` on tombstones, or when the read was projected
    /// to [`STATE_COLUMNS`].
    pub body: Option<String>,
    /// The full metadata object as JSON; `None` like `body`.
    pub meta_json: Option<String>,
    /// The row's operation.
    pub op: Op,
    /// When the writer observed this state (epoch milliseconds).
    pub observed_at: i64,
}

/// Decode one record batch into rows, appending to `out`. `with_payload` says
/// whether `body`/`meta_json` are expected in the batch (a state-projected
/// scan omits them).
pub(crate) fn rows_from_batch(
    batch: &RecordBatch,
    with_payload: bool,
    out: &mut Vec<LakeRow>,
) -> Result<()> {
    let external_id = string_column(batch, "external_id")?;
    let content_hash = string_column(batch, "content_hash")?;
    let op_col = string_column(batch, "op")?;
    let observed_at = long_column(batch, "observed_at")?;
    let payload = if with_payload {
        Some((string_column(batch, "body")?, string_column(batch, "meta_json")?))
    } else {
        None
    };

    out.reserve(batch.num_rows());
    for row in 0..batch.num_rows() {
        let op = match non_null_str(op_col, row, "op")? {
            OP_UPSERT => Op::Upsert,
            OP_DELETE => Op::Delete,
            other => return BadOpSnafu { value: other.to_owned(), row }.fail(),
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
                return NullValueSnafu { column: "content_hash", row }.fail();
            }
            if with_payload && body.is_none() {
                return NullValueSnafu { column: "body", row }.fail();
            }
            if with_payload && meta_json.is_none() {
                return NullValueSnafu { column: "meta_json", row }.fail();
            }
        }
        out.push(LakeRow {
            external_id: non_null_str(external_id, row, "external_id")?.to_owned(),
            content_hash: opt(content_hash),
            body,
            meta_json,
            op,
            observed_at: observed_at
                .is_valid(row)
                .then(|| observed_at.value(row))
                .context(NullValueSnafu { column: "observed_at", row })?,
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
pub(crate) fn document_from_row(row: LakeRow) -> Result<Document> {
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

/// Keep, per `external_id`, only the row with the greatest `observed_at`
/// (ties: the later-read row). Per-slice writers are serialized by the fleet's
/// oneshot units, so within a slice `observed_at` only moves forward; across
/// slices the rule is "any writer's most recent observation wins".
pub(crate) fn fold_latest(rows: Vec<LakeRow>) -> HashMap<String, LakeRow> {
    let mut latest: HashMap<String, LakeRow> = HashMap::new();
    for row in rows {
        match latest.get(&row.external_id) {
            Some(existing) if existing.observed_at > row.observed_at => {}
            _ => {
                latest.insert(row.external_id.clone(), row);
            }
        }
    }
    latest
}

/// Borrow one column as a `StringArray`, erroring (never defaulting) when the
/// column is absent or mis-typed.
fn string_column<'a>(batch: &'a RecordBatch, column: &'static str) -> Result<&'a StringArray> {
    let array = batch.column_by_name(column).context(MissingColumnSnafu { column })?;
    array.as_any().downcast_ref::<StringArray>().context(ColumnTypeSnafu { column })
}

/// Borrow one column as an `Int64Array`, erroring like [`string_column`].
fn long_column<'a>(batch: &'a RecordBatch, column: &'static str) -> Result<&'a Int64Array> {
    let array = batch.column_by_name(column).context(MissingColumnSnafu { column })?;
    array.as_any().downcast_ref::<Int64Array>().context(ColumnTypeSnafu { column })
}

/// Read one row of a required string column, erroring on a null cell.
fn non_null_str<'a>(array: &'a StringArray, row: usize, column: &'static str) -> Result<&'a str> {
    array.is_valid(row).then(|| array.value(row)).context(NullValueSnafu { column, row })
}
