"""Write distilled items as a corpus parquet slice + manifest.

This mirrors the contract of ``packages/sink/parquet`` exactly so the hourly
leader fold (``fold_parquet_into_lake``) ingests the slice with zero Rust
changes and the view reconcile publishes it to Mixedbread:

- 9-column schema: external_id, source, content_hash, title, url, host,
  timestamp (int64, epoch seconds), body, meta_json -- all Utf8 but timestamp;
  external_id/source/content_hash/body/meta_json non-nullable.
- ``content_hash`` is exactly ``sha256:<hex>`` of the embedded body bytes
  (``source_meta::hash_body``).
- ``_manifest.json`` is ``{"content_hash": sha256 over the sorted set of
  (external_id NUL content_hash NUL) pairs}`` (``corpus_hash``).
- ``meta_json`` is a flat object carrying source/external_id/content_hash/
  title plus the standard filter keys (user/host/project/timestamp) so the
  slice rides existing query filters; <=128 KiB / <=256 keys per record.

The file is written with pyarrow (not polars) so the embedded Arrow schema
says Utf8, which is what the Rust reader downcasts to (``StringArray``).
"""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

import pyarrow as pa
import pyarrow.parquet as pq

SOURCE = "distilled_facts"
# Per-session outcome verdicts (ENG-2710) ride a sibling slice with the same
# contract; the leader fold ingests any (host, user, source) slice generically.
SESSIONS_SOURCE = "session_outcomes"
MAX_METADATA_BYTES = 128 * 1024
MAX_METADATA_KEYS = 256

SCHEMA = pa.schema(
    [
        pa.field("external_id", pa.string(), nullable=False),
        pa.field("source", pa.string(), nullable=False),
        pa.field("content_hash", pa.string(), nullable=False),
        pa.field("title", pa.string(), nullable=True),
        pa.field("url", pa.string(), nullable=True),
        pa.field("host", pa.string(), nullable=True),
        pa.field("timestamp", pa.int64(), nullable=True),
        pa.field("body", pa.string(), nullable=False),
        pa.field("meta_json", pa.string(), nullable=False),
    ]
)

REQUIRED_NON_NULL = ("external_id", "source", "content_hash", "body", "meta_json")


class ContractError(ValueError):
    """The slice violates the parquet corpus contract."""


def hash_body(body: bytes) -> str:
    return "sha256:" + hashlib.sha256(body).hexdigest()


def corpus_hash(pairs: list[tuple[str, str]]) -> str:
    """sha256 over the sorted (external_id, content_hash) set, NUL-separated.

    Byte-for-byte the same construction as sink-parquet's ``corpus_hash``.
    """

    digest = hashlib.sha256()
    for external_id, content_hash in sorted(set(pairs)):
        digest.update(external_id.encode())
        digest.update(b"\x00")
        digest.update(content_hash.encode())
        digest.update(b"\x00")
    return digest.hexdigest()


def item_body(item: dict, project: str, session_labels: list[str] | None = None) -> str:
    """The embedded, self-contained fact text (what gets hashed + indexed)."""
    trailer = (
        f"(outcome: {item['outcome']}; scope: {item['scope']}; project: {project};"
        f" sessions: {', '.join(item.get('sessions', [])[:6]) or 'n/a'}"
    )
    if session_labels:
        trailer += f"; session-labels: {', '.join(session_labels)}"
    lines = [
        f"# {item['title']}",
        "",
        item["body"],
        "",
        trailer + ")",
    ]
    return "\n".join(lines)


def item_row(
    item: dict,
    project: str,
    host: str,
    user: str,
    session_labels: dict[str, str] | None = None,
) -> dict:
    # ``session_labels`` maps evidence session ids to their outcome verdicts;
    # lessons whose evidence includes a failed session are the most valuable
    # guardrails, so the labels ride the body and meta (``failure_derived``).
    labels = sorted(
        {
            session_labels[sid]
            for sid in item.get("sessions", [])
            if session_labels and sid in session_labels
        }
    )
    slug_source = project.strip("/").replace("/", "-") or "unknown"
    external_id = f"{SOURCE}:{user}:{slug_source}:{item['id']}"
    body = item_body(item, project, session_labels=labels)
    content_hash = hash_body(body.encode())
    timestamp = int(item.get("last_updated") or 0) or None
    scope = item.get("scope", "shared")
    meta = {
        "source": SOURCE,
        "external_id": external_id,
        "content_hash": content_hash,
        "title": item["title"],
        "host": host,
        "user": user,
        "project": project,
        "scope": f"user:{user}" if scope == "user" else "shared",
        "outcome": item.get("outcome", "mixed"),
        "session_ids": ",".join(item.get("sessions", [])[:16]),
        "item_id": item["id"],
    }
    if timestamp is not None:
        meta["timestamp"] = timestamp
    if item.get("evidence_from"):
        meta["evidence_from"] = int(item["evidence_from"])
    if item.get("evidence_to"):
        meta["evidence_to"] = int(item["evidence_to"])
    if labels:
        meta["session_labels"] = ",".join(labels)
        meta["failure_derived"] = "failure" in labels
    return {
        "external_id": external_id,
        "source": SOURCE,
        "content_hash": content_hash,
        "title": item["title"],
        "url": None,
        "host": host,
        "timestamp": timestamp,
        "body": body,
        "meta_json": _encode_meta(meta, external_id),
    }


def _encode_meta(meta: dict, external_id: str) -> str:
    meta_json = json.dumps(meta, sort_keys=True)
    if len(meta_json.encode()) > MAX_METADATA_BYTES:
        raise ContractError(f"meta_json for {external_id} exceeds {MAX_METADATA_BYTES} bytes")
    if len(meta) > MAX_METADATA_KEYS:
        raise ContractError(f"meta_json for {external_id} exceeds {MAX_METADATA_KEYS} keys")
    return meta_json


def _clip_chars(text: str, limit: int) -> str:
    text = " ".join(text.split())
    if len(text) <= limit:
        return text
    return text[: limit - 1] + "…"


def session_body(session_id: str, rec: dict, project: str) -> str:
    """Self-contained outcome record text: reason first, then key stats."""
    label = rec.get("label") or "partial"
    goal = rec.get("goal") or "(no goal recorded)"
    stats = (
        f"label: {label}; turns: {int(rec.get('turns') or 0)};"
        f" duration_s: {int(rec.get('duration_s') or 0)};"
        f" models: {', '.join(rec.get('models') or []) or 'unknown'};"
        f" tool-errors: {int(rec.get('errors') or 0)};"
        f" user-corrections: {int(rec.get('corrections') or 0)}"
    )
    lines = [
        f"# [{label}] {_clip_chars(goal, 200)}",
        "",
        rec.get("reason") or "(no reason recorded)",
        "",
        f"({stats}; project: {project}; session: {session_id})",
    ]
    return "\n".join(lines)


def session_row(session_id: str, rec: dict, project: str, host: str, user: str) -> dict:
    """One 9-column row for a judged session (``source=session_outcomes``)."""
    slug_source = project.strip("/").replace("/", "-") or "unknown"
    external_id = f"{SESSIONS_SOURCE}:{user}:{slug_source}:{session_id}"
    body = session_body(session_id, rec, project)
    content_hash = hash_body(body.encode())
    timestamp = int(rec.get("last_ts") or 0) or None
    label = rec.get("label") or "partial"
    title = f"[{label}] {_clip_chars(rec.get('goal') or session_id, 140)}"
    meta = {
        "source": SESSIONS_SOURCE,
        "external_id": external_id,
        "content_hash": content_hash,
        "title": title,
        "host": host,
        "user": user,
        "project": project,
        "session_id": session_id,
        "label": label,
        "reason": rec.get("reason") or "",
        "turns": int(rec.get("turns") or 0),
        "duration_s": int(rec.get("duration_s") or 0),
        "models": ",".join(rec.get("models") or []),
    }
    if timestamp is not None:
        meta["timestamp"] = timestamp
    return {
        "external_id": external_id,
        "source": SESSIONS_SOURCE,
        "content_hash": content_hash,
        "title": title,
        "url": None,
        "host": host,
        "timestamp": timestamp,
        "body": body,
        "meta_json": _encode_meta(meta, external_id),
    }


def write_slice(rows: list[dict], slice_dir: Path) -> dict[str, Path]:
    """Write ``data.parquet`` + ``_manifest.json`` for one slice directory.

    Mirrors the sink's empty-set policy: an empty row set writes nothing
    (never a wipe).
    """

    if not rows:
        return {}
    slice_dir.mkdir(parents=True, exist_ok=True)
    columns = {name: [row[name] for row in rows] for name in SCHEMA.names}
    table = pa.Table.from_pydict(columns, schema=SCHEMA)
    data_path = slice_dir / "data.parquet"
    manifest_path = slice_dir / "_manifest.json"
    pq.write_table(table, data_path)
    manifest = {
        "content_hash": corpus_hash([(r["external_id"], r["content_hash"]) for r in rows])
    }
    manifest_path.write_text(json.dumps(manifest))
    return {"data": data_path, "manifest": manifest_path}


def validate_slice(slice_dir: Path, source: str = SOURCE) -> int:
    """Re-read the slice with polars and assert the full contract.

    Returns the row count. Raises :class:`ContractError` on any violation.
    Validation deliberately goes through a second reader (polars) so a
    pyarrow-side encoding quirk cannot self-certify.
    """

    import polars as pl

    data_path = slice_dir / "data.parquet"
    manifest_path = slice_dir / "_manifest.json"
    if not data_path.is_file() or not manifest_path.is_file():
        raise ContractError(f"missing data.parquet or _manifest.json in {slice_dir}")

    frame = pl.read_parquet(data_path)
    expected = {
        "external_id": pl.String,
        "source": pl.String,
        "content_hash": pl.String,
        "title": pl.String,
        "url": pl.String,
        "host": pl.String,
        "timestamp": pl.Int64,
        "body": pl.String,
        "meta_json": pl.String,
    }
    if list(frame.columns) != list(expected):
        raise ContractError(f"column mismatch: {frame.columns} != {list(expected)}")
    for name, dtype in expected.items():
        if frame.schema[name] != dtype:
            raise ContractError(f"column {name} is {frame.schema[name]}, want {dtype}")
    for name in REQUIRED_NON_NULL:
        nulls = frame[name].null_count()
        if nulls:
            raise ContractError(f"column {name} has {nulls} nulls")

    pairs: list[tuple[str, str]] = []
    for row in frame.iter_rows(named=True):
        body_hash = hash_body(row["body"].encode())
        if row["content_hash"] != body_hash:
            raise ContractError(
                f"{row['external_id']}: content_hash {row['content_hash']} != {body_hash}"
            )
        if row["source"] != source:
            raise ContractError(f"{row['external_id']}: source {row['source']!r}")
        meta = json.loads(row["meta_json"])
        if not isinstance(meta, dict):
            raise ContractError(f"{row['external_id']}: meta_json is not an object")
        for key in ("source", "external_id", "content_hash", "title"):
            if key not in meta:
                raise ContractError(f"{row['external_id']}: meta_json missing {key}")
        if meta["external_id"] != row["external_id"] or meta["content_hash"] != row["content_hash"]:
            raise ContractError(f"{row['external_id']}: meta_json identity mismatch")
        if len(row["meta_json"].encode()) > MAX_METADATA_BYTES:
            raise ContractError(f"{row['external_id']}: meta_json too large")
        if len(meta) > MAX_METADATA_KEYS:
            raise ContractError(f"{row['external_id']}: meta_json too many keys")
        pairs.append((row["external_id"], row["content_hash"]))

    manifest = json.loads(manifest_path.read_text())
    expected_hash = corpus_hash(pairs)
    if manifest.get("content_hash") != expected_hash:
        raise ContractError(
            f"manifest hash {manifest.get('content_hash')} != recomputed {expected_hash}"
        )
    # The Rust reader requires Utf8 (not LargeUtf8/Utf8View) string columns;
    # check the physical arrow schema pyarrow reads back.
    arrow_schema = pq.read_schema(data_path)
    for name in SCHEMA.names:
        got = arrow_schema.field(name).type
        want = SCHEMA.field(name).type
        if got != want:
            raise ContractError(f"arrow type of {name} is {got}, want {want}")
    return frame.height
