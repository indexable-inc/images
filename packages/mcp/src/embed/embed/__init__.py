"""Code embeddings for the ix-mcp kernel: chunk, embed, cache, search.

Bundled like ``view``/``nix``/``fsearch`` so every session can ``import embed``
with no setup. Inference is Python-native and in-process: a resident
``SentenceTransformer`` (Qwen/Qwen3-Embedding-0.6B) on torch/MPS fp16, measured
at ~12.3k tok/s on an M5 Max -- roughly 2x a llama.cpp Metal server pipeline on
the same corpus, with 0.9998 mean-cosine agreement (index#3417). No server
process, no HTTP seam.

* :func:`texts` -- the raw encoder: ``list[str]`` in, unit-normalized
  ``float32[n, 1024]`` out. The model lazy-loads on first call (~17 s, one
  download on the very first use) and stays resident.
* :func:`ensure` -- chunk a repo at function level, embed only the chunks whose
  content hash misses the parquet cache, upsert, and return the full frame.
  The content-hash cache is the incrementality story: a typical edit re-embeds
  dozens of functions (seconds), never the corpus (~2.5 min cold).
* :func:`similar` -- semantic code search: the cached chunks nearest a query
  text (or a file's contents), one normalized GEMM over the cache matrix.
* :func:`pairs` -- near-duplicate mining over the whole cache: the top
  all-pairs cosine hits, the candidate band for type-4 (semantic) clone review.
* :func:`dupes` -- the one-call duplicate-code finder: :func:`ensure` a root,
  then mine the top pairs among that root's own chunks only.

The cache is one parquet file per model revision at
``~/.cache/index-embed/<model_rev>.parquet`` (columns ``hash``, ``path``,
``embedding: array[f32, 1024]``); a model bump changes the revision and so
starts a fresh file, never mixing vector spaces.

Every call here is synchronous and compute-bound; from a kernel cell run the
long ones in a thread so the shared event loop stays free::

    frame = await asyncio.to_thread(embed.ensure, ".")
    hits = await asyncio.to_thread(embed.similar, "capped exponential backoff")

The chunker is a deliberate regex port of the measured prototype (fn/def
extraction for Rust and Python, a ``// <path> :: <signature>`` header line,
8000-char truncation, blake2b-16 content hash). The tree-sitter chunker over
``clone_hash::significant_nodes`` supersedes it (index#3423).
"""

from __future__ import annotations

import hashlib
import heapq
import re
from pathlib import Path
from typing import TYPE_CHECKING

import numpy as np
import polars as pl

if TYPE_CHECKING:
    import numpy.typing as npt
    from sentence_transformers import SentenceTransformer  # type: ignore[import-not-found]  # darwin-only optional dep (index#3417); zuban ignores per-module config

__all__ = ["EmbedError", "dupes", "ensure", "pairs", "similar", "texts"]

__version__ = "0.1.0"

MODEL_ID = "Qwen/Qwen3-Embedding-0.6B"
DIM = 1024
MAX_SEQ_LENGTH = 2048
BATCH_SIZE = 32
# ~2k tokens: chunks past this are truncated so one pathological function
# cannot dominate batch latency. Matches the measured prototype.
MAX_CHUNK_CHARS = 8000
CACHE_DIR = Path("~/.cache/index-embed")
# Pair-mining row-block height: bounds the sims slice at block x n float32
# (~1 GB at 123k chunks) instead of the fatal full n x n (index#3498).
_MINE_BLOCK_ROWS = 2048


class EmbedError(Exception):
    """The embedder cannot proceed (missing dependency, empty cache, bad input)."""


_model: SentenceTransformer | None = None


def _load_model() -> SentenceTransformer:
    """The resident model, loaded on first use and kept for the session."""
    global _model
    if _model is not None:
        return _model
    try:
        from sentence_transformers import SentenceTransformer  # type: ignore[import-not-found]  # darwin-only optional dep (index#3417); zuban ignores per-module config
    except ImportError as exc:
        raise EmbedError(
            "embed: `sentence-transformers` (with torch) is not importable; it is "
            "bundled into the ix-mcp interpreter on macOS only (inference runs on "
            "torch/MPS). On this interpreter, `pip install sentence-transformers` "
            "or use a darwin session."
        ) from exc
    model = SentenceTransformer(
        MODEL_ID,
        device="mps",
        model_kwargs={"torch_dtype": "float16"},
        # sentence-transformers 5.4 renamed tokenizer_kwargs to processor_kwargs.
        processor_kwargs={"model_max_length": MAX_SEQ_LENGTH},
    )
    model.max_seq_length = MAX_SEQ_LENGTH
    _model = model
    return model


def _model_rev() -> str:
    """The model weights identity: the HF commit the local snapshot resolved to.

    Read from the hub cache's ``refs/main`` file; the first call on a machine
    without the snapshot loads the model (which downloads it and writes the
    ref). Names the cache file, so two revisions never share vectors.
    """
    try:
        from huggingface_hub import constants  # type: ignore[import-not-found]  # darwin-only optional dep (index#3417); zuban ignores per-module config
    except ImportError as exc:
        raise EmbedError(
            "embed: `huggingface_hub` is not importable; it ships with "
            "sentence-transformers, so this interpreter lacks the embed "
            "dependencies (bundled on macOS only)."
        ) from exc
    ref = Path(constants.HF_HUB_CACHE) / f"models--{MODEL_ID.replace('/', '--')}" / "refs" / "main"
    if not ref.is_file():
        _load_model()
    if not ref.is_file():
        raise EmbedError(f"embed: model snapshot loaded but no ref file at {ref}")
    return ref.read_text(encoding="utf-8").strip()


def texts(items: list[str]) -> npt.NDArray[np.float32]:
    """Embed ``items`` into unit-normalized ``float32[n, 1024]`` rows."""
    model = _load_model()
    vectors = model.encode(
        items,
        batch_size=BATCH_SIZE,
        show_progress_bar=False,
        normalize_embeddings=True,
    )
    return np.asarray(vectors, dtype=np.float32)


# ---------------------------------------------------------------------------
# Chunking (regex prototype port; see the module docstring for the successor)

_SKIP_DIRS = {".git", "target", "node_modules", ".direnv", "result", "vendor"}
_RUST_FN = re.compile(
    r'^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const\s+|async\s+|unsafe\s+|extern\s+"[^"]*"\s+)*fn\s+\w+',
    re.MULTILINE,
)
_PY_DEF = re.compile(r"^(?:async\s+)?def\s+\w+", re.MULTILINE)
# A chunk keeps at least this many newlines (5 lines): shorter bodies are
# mostly signature noise and embed poorly.
_MIN_BODY_NEWLINES = 4
# Brace-matching scan cap: a pathological unclosed brace stops here instead of
# scanning the whole file per match.
_BRACE_SCAN_CAP = 200_000


def _rust_body(src: str, start: int) -> str | None:
    """The braced body from a ``fn`` match, or None for a bodyless declaration."""
    brace = src.find("{", start)
    if brace == -1:
        return None
    if ";" in src[start:brace]:  # a trait/extern declaration, not a definition
        return None
    depth = 0
    for i in range(brace, min(len(src), brace + _BRACE_SCAN_CAP)):
        char = src[i]
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return src[start : i + 1]
    return None


def _py_body(src: str, start: int) -> str:
    """The indented suite from a ``def`` match, ended by the first dedent."""
    lines = src[start:].split("\n")
    head_indent = len(lines[0]) - len(lines[0].lstrip())
    body = [lines[0]]
    for line in lines[1:]:
        stripped = line.lstrip()
        if stripped and (len(line) - len(stripped)) <= head_indent and not stripped.startswith(("#", ")", "]")):
            break
        body.append(line)
    return "\n".join(body).rstrip()


def _chunks(root: str) -> pl.DataFrame:
    """Function-level chunks under ``root``: one row per unique content hash.

    Columns ``hash`` (blake2b-16 hex of the final text), ``path`` (relative to
    ``root``), ``text`` (a ``// <path> :: <signature>`` header line plus the
    body, truncated to :data:`MAX_CHUNK_CHARS`).
    """
    base = Path(root).expanduser()
    if not base.is_dir():
        raise EmbedError(f"embed: root {str(base)!r} is not a directory")
    hashes: list[str] = []
    paths: list[str] = []
    chunk_texts: list[str] = []
    seen: set[str] = set()
    for dirpath, dirnames, filenames in base.walk():
        dirnames[:] = sorted(d for d in dirnames if d not in _SKIP_DIRS)
        for fname in sorted(filenames):
            if fname.endswith(".rs"):
                rust = True
            elif fname.endswith(".py"):
                rust = False
            else:
                continue
            fpath = dirpath / fname
            try:
                src = fpath.read_text(encoding="utf-8")
            except (UnicodeDecodeError, OSError):
                continue
            rel = str(fpath.relative_to(base))
            matcher = _RUST_FN if rust else _PY_DEF
            for match in matcher.finditer(src):
                body = _rust_body(src, match.start()) if rust else _py_body(src, match.start())
                if body is None or body.count("\n") < _MIN_BODY_NEWLINES:
                    continue
                signature = body.split("\n", 1)[0].strip()
                chunk = (f"// {rel} :: {signature}\n" + body)[:MAX_CHUNK_CHARS]
                digest = hashlib.blake2b(chunk.encode(), digest_size=16).hexdigest()
                if digest in seen:
                    continue
                seen.add(digest)
                hashes.append(digest)
                paths.append(rel)
                chunk_texts.append(chunk)
    return pl.DataFrame({"hash": hashes, "path": paths, "text": chunk_texts})


# ---------------------------------------------------------------------------
# Cache (one parquet file per model revision)


def _cache_path() -> Path:
    return CACHE_DIR.expanduser() / f"{_model_rev()}.parquet"


def _cache_schema() -> dict[str, pl.DataType]:
    return {"hash": pl.Utf8(), "path": pl.Utf8(), "embedding": pl.Array(pl.Float32, DIM)}


def _read_cache() -> pl.DataFrame:
    path = _cache_path()
    if path.is_file():
        return pl.read_parquet(path)
    return pl.DataFrame(schema=_cache_schema())


def _matrix(cache: pl.DataFrame) -> npt.NDArray[np.float32]:
    """The cache's embedding column as a dense ``float32[n, 1024]`` matrix.

    Rows are stored unit-normalized (``texts`` normalizes at encode time), so a
    plain GEMM over this matrix IS cosine similarity.
    """
    return cache.get_column("embedding").to_numpy().astype(np.float32, copy=False)


def ensure(root: str = ".") -> pl.DataFrame:
    """Chunk ``root``, embed the cache misses, upsert, return the full frame.

    The anti-join on content hash keeps repeat runs off the GPU entirely; only
    new or edited functions embed. Returns one row per unique chunk under
    ``root`` with columns ``hash``, ``path``, ``text``, ``embedding``.
    """
    chunk_frame = _chunks(root)
    cache = _read_cache()
    misses = chunk_frame.join(cache, on="hash", how="anti")
    if misses.height:
        vectors = texts(misses.get_column("text").to_list())
        fresh = misses.select("hash", "path").with_columns(
            pl.Series("embedding", vectors, dtype=pl.Array(pl.Float32, DIM))
        )
        cache = pl.concat([cache, fresh]).unique(subset="hash", keep="last")
        target = _cache_path()
        target.parent.mkdir(parents=True, exist_ok=True)
        staging = target.with_suffix(".parquet.tmp")
        cache.write_parquet(staging)
        staging.replace(target)
    return chunk_frame.join(cache.select("hash", "embedding"), on="hash", how="inner")


# ---------------------------------------------------------------------------
# Query (numpy is the vector database at this corpus size)


def similar(text_or_path: str, k: int = 10) -> pl.DataFrame:
    """The ``k`` cached chunks most similar to a query text (or a file's contents).

    Returns ``hash`` / ``path`` / ``score`` sorted by descending cosine.
    """
    query = text_or_path
    if "\n" not in query and len(query) < 1024:
        candidate = Path(query).expanduser()
        try:
            if candidate.is_file():
                query = candidate.read_text(encoding="utf-8")
        except OSError:
            pass
    cache = _read_cache()
    if cache.height == 0:
        raise EmbedError("embed: the cache is empty; run embed.ensure(root) first")
    vector = texts([query[:MAX_CHUNK_CHARS]])[0]
    scores = _matrix(cache) @ vector
    return (
        cache.select("hash", "path")
        .with_columns(pl.Series("score", scores))
        .sort("score", descending=True)
        .head(k)
    )


def _mine_pairs(frame: pl.DataFrame, k: int) -> pl.DataFrame:
    """The ``k`` highest-cosine distinct chunk pairs in ``frame``.

    ``frame`` needs ``hash`` / ``path`` / ``embedding`` columns; the result is
    ``path_a`` / ``path_b`` / ``hash_a`` / ``hash_b`` / ``score`` sorted by
    descending cosine over the strict upper triangle. Mines in row blocks
    feeding a global top-k heap: the full ``n x n`` sims matrix plus
    ``np.triu_indices`` would be ~183 GB at the index repo's 123k chunks
    (index#3498), so memory stays O(block * n).
    """
    if frame.height < 2:
        raise EmbedError("embed: fewer than two chunks; run embed.ensure(root) first")
    matrix = frame.get_column("embedding").to_numpy().astype(np.float32, copy=False)
    n = matrix.shape[0]
    heap: list[tuple[float, int, int]] = []
    cols = np.arange(n)[None, :]
    for start in range(0, n, _MINE_BLOCK_ROWS):
        block = matrix[start : start + _MINE_BLOCK_ROWS]
        sims = block @ matrix.T
        rows_global = (np.arange(block.shape[0]) + start)[:, None]
        sims[cols <= rows_global] = -np.inf  # strict upper triangle only
        flat = sims.ravel()
        m = min(k, flat.size)
        top = np.argpartition(-flat, m - 1)[:m]
        for f in top:
            score = float(flat[f])
            if score == -np.inf:
                continue
            i = start + int(f) // n
            j = int(f) % n
            if len(heap) < k:
                heapq.heappush(heap, (score, i, j))
            elif score > heap[0][0]:
                heapq.heapreplace(heap, (score, i, j))
    heap.sort(reverse=True)
    paths = frame.get_column("path")
    hashes = frame.get_column("hash")
    return pl.DataFrame(
        {
            "path_a": [paths[i] for _, i, _ in heap],
            "path_b": [paths[j] for _, _, j in heap],
            "hash_a": [hashes[i] for _, i, _ in heap],
            "hash_b": [hashes[j] for _, _, j in heap],
            "score": [s for s, _, _ in heap],
        }
    )


def pairs(k: int = 10) -> pl.DataFrame:
    """The ``k`` most similar chunk pairs across the whole cache: duplicate / type-4 clone candidates.

    Mines every repo ever embedded on this machine; for duplicates within one
    repo call :func:`dupes`, which scopes the mining to that root. Returns
    ``path_a`` / ``path_b`` / ``hash_a`` / ``hash_b`` / ``score`` sorted by
    descending cosine.
    """
    return _mine_pairs(_read_cache(), k)


def dupes(root: str = ".", k: int = 20) -> pl.DataFrame:
    """Find duplicate code under ``root`` in one call: the ``k`` most similar function pairs.

    Chunks and embeds ``root`` via :func:`ensure` (incremental: cached chunks
    skip the GPU), then mines the top cosine pairs among ``root``'s own chunks
    only. Scores ~0.95+ read as near-verbatim duplication; ~0.85-0.95 as
    same-shape logic worth a shared helper -- the type-4 (semantic) clones the
    AST ``nix run .#clone`` gate cannot see. Returns ``path_a`` / ``sig_a`` /
    ``path_b`` / ``sig_b`` / ``score`` sorted by descending cosine.
    """
    frame = ensure(root)
    header = pl.col("text").str.split("\n").list.first()
    sigs = frame.select("hash", header.str.replace(r"^// .*? :: ", "").alias("sig"))
    return (
        _mine_pairs(frame, k)
        .join(sigs.rename({"hash": "hash_a", "sig": "sig_a"}), on="hash_a")
        .join(sigs.rename({"hash": "hash_b", "sig": "sig_b"}), on="hash_b")
        .select("path_a", "sig_a", "path_b", "sig_b", "score")
        .sort("score", descending=True)
    )
