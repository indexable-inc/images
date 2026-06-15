# source-github

Turns a GitHub export into embeddable search documents. It reads a directory of
JSON produced by [`export.sh`](./export.sh) and projects each issue, pull
request, and failed CI run into one [`source_meta`](../meta) `Document`,
queryable through [`search`](../../search) as `--source github`.

The crate is pure: it reads three files and does no network or process I/O. The
network work (driving the `gh` CLI, joining a PR's inline review threads and a
CI run's failed jobs from separate endpoints) lives in the export script, so the
adapter never has to do a join.

## Export

```sh
./export.sh ./export indexable-inc/index acme/widgets
```

This writes:

- `export/metadata.json`: provenance (`exported_at`, `ci_since`) and the repos
  covered.
- `export/items.json`: one combined array of issues and pull requests. Each
  element carries its own `repo` and a `kind` (`issue` or `pr`). Pull requests
  nest their `reviews` and inline `review_threads` in place.
- `export/ci_runs.json`: one combined array of failed CI runs — completed
  workflow runs from the last `CI_WINDOW_DAYS` days (default 90) whose
  conclusion is `failure`, `timed_out`, or `cancelled` — each nesting its
  `failed_jobs` (with the failed step names) in place. The window is deliberate:
  failed runs lose diagnostic value once the flake is fixed, and it bounds the
  API walk on a busy repo. The adapter treats a missing `ci_runs.json` as
  empty, so exports written before the CI pass stay readable.

Requires `gh` (authenticated) and `jq`.

### Cost and scale

The `gh issue/pr list` calls return only the first page of each nested
connection (comments, reviews), so the export does not read comments or reviews
from them. Instead it fetches, fully paginated, per item: conversation comments
(one call per issue and per PR), and for each PR its reviews and inline review
threads (two more calls). That is one call per issue and three per PR, run in
parallel; set `EXPORT_JOBS` to tune the parallelism (default 8).

The CI pass lists failed runs server-side filtered per conclusion (so green
runs are never paginated through), then makes one `actions/runs/<id>/jobs` call
per failed run, with the same parallelism. A repo with thousands of recent
failures makes thousands of calls; shrink `CI_WINDOW_DAYS` to bound it.

The tradeoff is completeness over call count: a repo with thousands of PRs makes
thousands of REST calls and can take a while or brush the GitHub rate limit.
That is deliberate, so no discussion is silently dropped. A future pass could
move to a GraphQL query that paginates the nested connections inline if the call
volume becomes a problem.

## Index

```sh
indexer --mixedbread-store my-store --github-export ./export
```

## Grain and identity

One document per issue, per pull request, and per failed CI run. The item
`external_id` is `github:<owner>/<repo>:<number>` (a `:` separator, not `#`, so
it survives the sink's delete path, which carries the id in a URL where `#`
would start a fragment), stable across re-exports, so the Mixedbread
sink reconciles in place: an edited item re-embeds and an unchanged one is
skipped (`sync_documents` keys on `external_id` + `content_hash`).

CI-run documents are `github:ci:<owner>/<repo>:<run_id>` with `kind=ci_run` in
the flat metadata, titled
`<repo> CI failure: <workflow> #<run_number> (<branch>)`. The body carries the
workflow, branch, head SHA, run URL, and each failed job with its failed step
names, so an agent staring at a red check can recall whether the same job/step
already failed (and how it was diagnosed) instead of re-deriving the flake.

## Known limitations

- The indexer pass uploads and updates; it does not delete on its own. An item
  that is deleted or dropped from a later export keeps its last-exported
  version in the store unless the indexer runs with `--gc`, which diffs the
  store's `github` records against the export just indexed and deletes the
  vanished ones (plus exact-duplicate file objects left by retried uploads).
  Re-exporting on a schedule keeps content fresh; pair it with `--gc` to prune
  removed items too.
- First pass is export-driven (like the Linear adapter). There is no live API
  ingestion, and Discussions and gists are out of scope.
- Inline review threads come from the REST `pulls/{n}/comments` endpoint, which
  does not expose resolved/outdated state. The body renders the thread location
  and comments, not whether the thread was resolved.
