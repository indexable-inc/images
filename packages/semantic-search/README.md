# semantic-search

Semantic code search over a [Mixedbread](https://www.mixedbread.com) store,
content-addressed so it deduplicates across git worktrees and never deletes one
checkout's files because of another. A daemon-free alternative to `mgrep` for
the multi-worktree case.

It is two crates: [`mixedbread`](../mixedbread) is a standalone async API client
with no domain logic; this crate owns the indexing, manifest, and search logic
and consumes that client behind a [`Store`] trait.

## Why it exists

`mgrep` keys each file by its absolute path and, on sync, deletes anything under
the synced path that is missing locally. Two checkouts that resolve to the same
path (containers, CI, devcontainers) then overwrite and delete each other, and
twenty worktrees of one repo pay the embedding cost twenty times.

This tool keys each file by the hash of its bytes instead:

- **Upload only new content.** A blob already in the store is never re-uploaded
  or re-embedded. The second through twentieth worktrees of a repo embed almost
  nothing.
- **Never delete during sync.** A stored entry is shared across checkouts, so
  deletion can only be decided by reference counting across every manifest. That
  belongs in a separate garbage-collection pass, not in ordinary sync, which
  removes the cross-worktree deletion footgun entirely.
- **Scope results to this checkout.** A local manifest records this worktree's
  `path -> hash` view. Search over-fetches from the shared store and keeps only
  hits whose hash is in the manifest, mapping each back to its local path.
- **No daemon, no sync flag.** Every run rebuilds the manifest cheaply (an
  unchanged file's mtime skips re-hashing), uploads only what is new, waits for
  it to embed, then searches. New files are picked up automatically at search
  time; `--no-sync` skips that for a pure offline search.

## Authentication

By default it resolves a credential the way you'd expect: `MXBAI_API_KEY` if
set, otherwise the token written by `mgrep login` at `~/.mgrep/token.json`. That
stored token is an OAuth access token, so it is exchanged for a short-lived API
JWT at the platform auth endpoint (the same exchange `mgrep` itself does). If
neither is present, it tells you to set the key or run `mgrep login`.

## Storage

Two places. The embeddings, file content, and metadata live remotely in the
Mixedbread store (keyed by content hash). The only local state is the manifest,
held in **one shared SQLite database** at `<cache>/semantic-search/index.db`
(`~/Library/Caches/...` on macOS, `~/.cache/...` on Linux), keyed by
`(root, rel_path)`. It is opened in WAL mode so several invocations can run at
once (concurrent readers, one writer), and it's a rebuildable cache: delete it
and the next run reconstructs it. SQLite was chosen over LMDB/redb/RocksDB/sled
for multi-process safety plus in-place compaction; the reasoning is recorded in
[`src/db.rs`](src/db.rs).

## Usage

```sh
# No extra setup if you already ran `mgrep login`.
export MXBAI_API_KEY=...           # optional, from https://mixedbread.com

# Search the current checkout. New/changed files are detected, uploaded, and
# embedded automatically before the search runs.
semantic-search "where is the retry backoff configured"

# Show matched content.
semantic-search -c "http client construction"

# Synthesize an answer with sources.
semantic-search -a "how does sync decide what to upload"

# Skip the index step and search the store as-is.
semantic-search --no-sync "anything"
```

Flags mirror `mgrep search` where they overlap: `-c/--content`, `-m/--max-count`,
`-a/--answer`, `--no-rerank`, `-w/--web`, `--agentic`, plus `--no-sync` to skip
auto-indexing. The store name comes from `--store` or `MXBAI_STORE` (default
`semantic-search`); the API base URL from `--base-url` or `MXBAI_BASE_URL`.

## Known limitations

- Content lives in one shared store, so without the eventual GC pass, blobs that
  no checkout references anymore are not reclaimed. Correctness over storage.
- When branches diverge, the differing files get distinct entries; search
  over-fetches and filters client-side rather than asking the server for an
  exact per-worktree set. Large divergence may need a higher `--max-count`.
- The live network paths (the two-step `/v1/files` upload and the `mgrep login`
  token-to-JWT exchange) need a real credential to exercise; the dedup,
  manifest, and search logic are covered by tests against an in-memory store.

[`Store`]: src/backend.rs
