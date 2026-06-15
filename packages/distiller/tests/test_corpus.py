"""Parquet-contract tests: schema, content_hash, manifest hash, validation."""

import hashlib
import json
from pathlib import Path

import pyarrow.parquet as pq
import pytest

from distiller import corpus


def make_item(i: int = 0, **overrides) -> dict:
    item = {
        "id": f"df-{i:012x}",
        "title": f"Lesson {i}",
        "body": f"Body of lesson {i}: run exactly `nix build .#thing`.",
        "outcome": "success",
        "scope": "shared",
        "sessions": ["sess-a", "sess-b"],
        "first_seen": 1_700_000_000.0,
        "last_updated": 1_700_000_100.0,
        "evidence_from": 1_699_999_000.0,
        "evidence_to": 1_700_000_050.0,
    }
    item.update(overrides)
    return item


def test_content_hash_is_sha256_of_body():
    row = corpus.item_row(make_item(), "/home/u/repo", "hostx", "useru")
    expected = "sha256:" + hashlib.sha256(row["body"].encode()).hexdigest()
    assert row["content_hash"] == expected
    assert row["content_hash"].startswith("sha256:")
    assert len(row["content_hash"]) == len("sha256:") + 64


def test_meta_json_carries_identity_and_filter_keys():
    row = corpus.item_row(make_item(scope="user"), "/home/u/repo", "hostx", "useru")
    meta = json.loads(row["meta_json"])
    for key in ("source", "external_id", "content_hash", "title"):
        assert meta[key] == row[key] or key == "title"
    assert meta["user"] == "useru"
    assert meta["host"] == "hostx"
    assert meta["project"] == "/home/u/repo"
    assert meta["timestamp"] == row["timestamp"] == 1_700_000_100
    assert meta["scope"] == "user:useru"
    assert row["external_id"].startswith("distilled_facts:useru:home-u-repo:df-")


def test_corpus_hash_matches_rust_construction():
    # sha256 over sorted (id \0 hash \0) pairs, duplicates collapsed.
    pairs = [("b", "h2"), ("a", "h1"), ("b", "h2")]
    digest = hashlib.sha256()
    for eid, ch in [("a", "h1"), ("b", "h2")]:
        digest.update(eid.encode())
        digest.update(b"\x00")
        digest.update(ch.encode())
        digest.update(b"\x00")
    assert corpus.corpus_hash(pairs) == digest.hexdigest()


def test_write_and_validate_roundtrip(tmp_path: Path):
    rows = [
        corpus.item_row(make_item(i), "/home/u/repo", "hostx", "useru") for i in range(3)
    ]
    paths = corpus.write_slice(rows, tmp_path / "slice")
    assert paths["data"].name == "data.parquet"
    assert paths["manifest"].name == "_manifest.json"
    assert corpus.validate_slice(tmp_path / "slice") == 3
    manifest = json.loads(paths["manifest"].read_text())
    assert manifest["content_hash"] == corpus.corpus_hash(
        [(r["external_id"], r["content_hash"]) for r in rows]
    )


def test_arrow_schema_is_utf8_not_large(tmp_path: Path):
    # The Rust reader downcasts to StringArray (Utf8); LargeUtf8/Utf8View
    # would fail its ColumnType check.
    rows = [corpus.item_row(make_item(), "/p", "h", "u")]
    corpus.write_slice(rows, tmp_path)
    schema = pq.read_schema(tmp_path / "data.parquet")
    import pyarrow as pa

    for name in ("external_id", "source", "content_hash", "body", "meta_json"):
        assert schema.field(name).type == pa.string()
        assert not schema.field(name).nullable
    assert schema.field("timestamp").type == pa.int64()


def test_validate_rejects_tampered_body(tmp_path: Path):
    rows = [corpus.item_row(make_item(i), "/p", "h", "u") for i in range(2)]
    corpus.write_slice(rows, tmp_path)
    # Rewrite with a body that no longer matches its content_hash.
    rows[0]["body"] = rows[0]["body"] + " TAMPERED"
    import pyarrow as pa

    table = pa.Table.from_pydict(
        {name: [r[name] for r in rows] for name in corpus.SCHEMA.names},
        schema=corpus.SCHEMA,
    )
    pq.write_table(table, tmp_path / "data.parquet")
    with pytest.raises(corpus.ContractError, match="content_hash"):
        corpus.validate_slice(tmp_path)


def test_validate_rejects_stale_manifest(tmp_path: Path):
    rows = [corpus.item_row(make_item(), "/p", "h", "u")]
    corpus.write_slice(rows, tmp_path)
    (tmp_path / "_manifest.json").write_text(json.dumps({"content_hash": "0" * 64}))
    with pytest.raises(corpus.ContractError, match="manifest hash"):
        corpus.validate_slice(tmp_path)


def test_empty_rows_never_wipe(tmp_path: Path):
    assert corpus.write_slice([], tmp_path / "slice") == {}
    assert not (tmp_path / "slice").exists()


def test_oversized_meta_rejected():
    item = make_item(title="t" * (200 * 1024))
    with pytest.raises(corpus.ContractError, match="bytes"):
        corpus.item_row(item, "/p", "h", "u")
