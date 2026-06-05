# search

Read-only semantic and regex search over the shared `index` corpus: one
[Mixedbread](https://www.mixedbread.com) store holding code plus agent/shell
history across the fleet.

`search` never indexes. The separate [`indexer`](../indexer) owns all ingestion
(code, Claude/Codex transcripts, shell history, Slack, Linear); this CLI only
queries the store it populates and projects the hits. Querying never reads or
uploads your local checkout, so running it from any directory is safe.

It is layered: [`mixedbread`](../mixedbread) is a standalone async API client
with no domain logic; [`search-core`](../search-core) owns the query and filter
logic behind a [`Store`] trait; this crate is the CLI over it, and
[`search-py`](../search-py) is the PyO3 binding (`import search`, also bundled
into the `ix-mcp` interpreter).

## Authentication

It resolves a credential the way you'd expect: `MXBAI_API_KEY` if set, otherwise
the token written by `mgrep login` at `~/.mgrep/token.json`. That stored token is
an OAuth access token, exchanged for a short-lived API JWT at the platform auth
endpoint (the same exchange `mgrep` itself does). If neither is present, it tells
you to set the key or run `mgrep login`.

## Usage

```sh
export MXBAI_API_KEY=...            # or run `mgrep login` once

# Search the whole corpus.
search "where is the retry backoff configured"

# Show matched content.
search -c "http client construction"

# Synthesize an answer with sources.
search -a "how does sync decide what to upload"

# Regex grep over the same corpus chunks.
search grep 'fn \w+\('

# Scope server-side: only my Claude history.
search --source claude_history --mine "deploy steps"

# Only code, one repository.
search --source code --repo indexable-inc/index "manifest reconcile"
```

Flags mirror `mgrep search` where they overlap: `-c/--content`, `-m/--max-count`,
`-a/--answer`, `--no-rerank`, `--reranker <model>` (defaults to the listwise
reranker), `-w/--web`, `--agentic`. The store name comes from
`--store` or `MXBAI_STORE` (default `index`); the API base URL from `--base-url`
or `MXBAI_BASE_URL`.

## Scope selectors

With no selector the whole corpus is searched; each selector narrows it
server-side (no local read). Repeatable, comma-joined values are accepted.

- `--source` / `--not-source`: include / exclude source tags (`code`,
  `claude_history`, `codex`, `shell`, `slack`, `linear`, `github`, `web`).
- `--repo`: restrict code to a repository slug, e.g. `indexable-inc/index`.
- `--user` / `--mine`: restrict records to these authors (or the current `$USER`).
- `--host`: restrict records to these machines.
- `--project`: restrict records to these project slugs (e.g. a Claude
  transcript's project directory).

## Known limitations

- Results are only as fresh and as broad as the last `indexer` run that
  populated the store; `search` adds nothing of its own.
- The live network paths (the search request and the `mgrep login` token-to-JWT
  exchange) need a real credential to exercise; the query and filter logic are
  covered by tests against an in-memory store.

[`Store`]: src/backend.rs
