# source-github

Turns a GitHub export into embeddable search documents. It reads a directory of
JSON produced by [`export.sh`](./export.sh) and projects each issue and pull
request into one [`source_meta`](../meta) `Document`, queryable through
[`search`](../../search) as `--source github`.

The crate is pure: it reads two files and does no network or process I/O. The
network work (driving the `gh` CLI, joining a PR's inline review threads from a
separate endpoint) lives in the export script, so the adapter never has to do a
join.

## Export

```sh
./export.sh ./export indexable-inc/index acme/widgets
```

This writes:

- `export/metadata.json`: provenance (`exported_at`) and the repos covered.
- `export/items.json`: one combined array of issues and pull requests. Each
  element carries its own `repo` and a `kind` (`issue` or `pr`). Pull requests
  nest their `reviews` and inline `review_threads` in place.

Requires `gh` (authenticated) and `jq`.

### Cost and scale

The `gh issue/pr list` calls return only the first page of each nested
connection (comments, reviews), so the export does not read comments or reviews
from them. Instead it fetches, fully paginated, per item: conversation comments
(one call per issue and per PR), and for each PR its reviews and inline review
threads (two more calls). That is one call per issue and three per PR, run in
parallel; set `EXPORT_JOBS` to tune the parallelism (default 8).

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

One document per issue and per pull request. The `external_id` is
`github:<owner>/<repo>:<number>` (a `:` separator, not `#`, so it survives the
sink's delete path, which carries the id in a URL where `#` would start a
fragment), stable across re-exports, so the Mixedbread
sink reconciles in place: an edited item re-embeds and an unchanged one is
skipped (`sync_documents` keys on `external_id` + `content_hash`).

## Known limitations

- The indexer pass uploads and updates; it does not delete. An item that is
  closed, deleted, or dropped from a later export keeps its last-exported
  version in the store until a separate garbage-collection pass runs against the
  `github` source. Re-exporting on a schedule keeps content fresh but does not
  prune removed items on its own.
- First pass is export-driven (like the Linear adapter). There is no live API
  ingestion, and Discussions and gists are out of scope.
- Inline review threads come from the REST `pulls/{n}/comments` endpoint, which
  does not expose resolved/outdated state. The body renders the thread location
  and comments, not whether the thread was resolved.
