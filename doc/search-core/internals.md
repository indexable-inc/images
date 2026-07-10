# search-core: query, projection, and filters

The read layer of [search-core](overview.md): how a backend result set becomes
displayable hits, how code is scoped to a worktree, how a recency feed and a
census are built, how a hit expands into its surrounding conversation, and how
scope selectors become a server-side metadata filter. Source files:
`src/search.rs`, `src/context.rs`, `src/query_filter.rs`, `src/repo.rs`.

## Projection (`src/search.rs`)

The store holds one shared entry per unique record across every source, so a raw
result can include code from other worktrees. `project` (`src/search.rs:553`)
decides what survives, per source (`src/search.rs:509`, `display_hit`):

- **Code** in `CodeScope::WorktreeExact` is kept only when its content hash is in
  this checkout's manifest, then relabeled to the local path; in
  `CodeScope::ServerFiltered` (a repo / all-repos query) the server filter
  already decided, so code passes through labeled by its stored path
  (`src/search.rs:528`).
- **Record sources** (slack, linear, claude_history, ...) have no checkout, so
  the server-side metadata filter is authoritative and they always pass through
  (`src/search.rs:544`).
- **Web** hits pass through only when the caller asked for them, line metadata
  stripped (`src/search.rs:517`).

[`overfetch`] (`src/search.rs:499`) asks the backend for `4*top_k` (min
`top_k+10`) so client-side filtering still leaves a full page. [`DisplayHit`]
(`src/search.rs:55`) is the projected hit; it serializes to the stable
`search --json` object (`label` renamed to `path`, absent provenance keys
skipped), the same shape the [`search-py`](../search-py/overview.md) dict uses.

[`RenderMode`] (`src/search.rs:36`): `Full` passes every chunk; `Compact`
collapses overlapping chunks of one document to the best-scoring one (refilling
from the overfetch buffer so `top_k` distinct documents still return) and caps
each snippet at `COMPACT_SNIPPET_CHARS = 400` (`src/search.rs:32`).

## Query verbs (`src/search.rs`)

- [`semantic`] (`src/search.rs:170`): search the store (plus the web store when
  `include_web`), project the hits.
- [`grep`] (`src/search.rs:198`): regex over the same chunks; local-corpus only.
- [`recent`] / [`ranked`] (`src/search.rs:224`): a metadata-only chunk listing
  sorted by descending `timestamp` (or any `SortBy`); no semantic scoring, so
  scores are the API placeholder.
- [`ask`] (`src/search.rs:390`): question-answering. The backend's
  `<cite i="N"/>` markers index its raw over-fetched source list; `align_citations`
  (`src/search.rs:432`) rewrites them to `[n]` over the projected `sources`,
  appending a cited-but-past-`top_k` hit so the citation still resolves and
  dropping a marker whose source was excluded ([`AnswerView`], `src/search.rs:139`).
- [`stats`] (`src/search.rs:319`): a per-source census. One discovery facet call
  finds which `source` tags exist (a bounded scan, `FACETS_MAX_FILES`), unioned
  with `KNOWN_SOURCE_TAGS`; each candidate is then counted by its own
  source-scoped facet call and probed for freshness with a single-chunk ranked
  listing, all concurrently. A source bigger than the scan bound reports the cap,
  marked [`SourceStat`]`.truncated` (`src/search.rs:282`).

## Conversation context (`src/context.rs`)

[`context`] (`src/context.rs:50`) expands one record into the conversation around
it. Given a hit's `external_id`, it lists the record's own chunks (resolving its
`session_id` and `timestamp`), then `window_around` (`src/context.rs:102`)
fetches `before` earlier and `after` later turns of the same session AND source
(a transcript shares its session id with its debug log, so both are scoped),
ordered by timestamp, one turn per record. Sources with no session (git, github,
code) fall back to the record's own chunks in document order; a bare session id
lists that session from its start (`src/context.rs:184`). [`ContextView`]
(`src/context.rs:28`) carries the turns and the anchor index.

## The metadata filter builder

[`FilterSpec`] (`src/query_filter.rs:13`) is the scope selectors a caller
applies: `sources` / `exclude_sources`, `repo`, `users`, `hosts`, `projects`,
and a `since`/`until` epoch-second window. [`build_filter`]
(`src/query_filter.rs:104`) turns it into a `mixedbread::Filter`: an `in` over
`source`, a `none` over excluded sources, `eq` on `repo`, `in` over user/host/
project, and inclusive `gte`/`lte` on `timestamp`, combined under `all` when
more than one clause is present, or `None` (search everything) when empty. One
builder, shared by the CLI, the Python binding, and the MCP tool, so the mapping
lives in one place (`src/query_filter.rs:1`).

[`parse_time_spec`] (`src/query_filter.rs:73`) accepts a bare integer (epoch
seconds) or a relative span `<n><unit>` with unit `s`/`m`/`h`/`d`/`w` resolved
against `now`, so both query edges accept the same `--since 7d` grammar.

## Repo slug (`src/repo.rs`)

[`repo_slug`] (`src/repo.rs:18`) derives a stable per-checkout name: the
`origin` remote URL parsed to `owner/repo` (so every worktree of one repo shares
a slug, `RepoSlug::Remote`), falling back to the directory name
(`RepoSlug::Local`) when there is no remote, never a silent empty string. This
is what scopes the sync dedup listing and the `--repo` query filter.

[`overfetch`]: #projection-srcsearchrs
[`DisplayHit`]: #projection-srcsearchrs
[`RenderMode`]: #projection-srcsearchrs
[`semantic`]: #query-verbs-srcsearchrs
[`grep`]: #query-verbs-srcsearchrs
[`recent`]: #query-verbs-srcsearchrs
[`ranked`]: #query-verbs-srcsearchrs
[`ask`]: #query-verbs-srcsearchrs
[`stats`]: #query-verbs-srcsearchrs
[`AnswerView`]: #query-verbs-srcsearchrs
[`SourceStat`]: #query-verbs-srcsearchrs
[`context`]: #conversation-context-srccontextrs
[`ContextView`]: #conversation-context-srccontextrs
[`FilterSpec`]: #the-metadata-filter-builder
[`build_filter`]: #the-metadata-filter-builder
[`parse_time_spec`]: #the-metadata-filter-builder
[`repo_slug`]: #repo-slug-srcrepors
