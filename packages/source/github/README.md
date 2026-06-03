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

Requires `gh` (authenticated) and `jq`. Inline review threads are fetched one
REST call per PR, run in parallel; set `EXPORT_JOBS` to tune the parallelism
(default 8).

## Index

```sh
indexer --mixedbread-store my-store --github-export ./export
```

## Grain and identity

One document per issue and per pull request. The `external_id` is
`github:<owner>/<repo>#<number>`, stable across re-exports, so the Mixedbread
sink reconciles in place: an edited item re-embeds, an unchanged one is skipped,
and a removed one is garbage-collected.

## Known limitations

- Garbage collection is scoped to the whole `github` source, not per repo. An
  export must cover the full set of repos you want indexed; dropping a repo from
  a later export deletes that repo's records from the store. Keep the repo list
  stable, or run separate stores per repo set.
- First pass is export-driven (like the Linear adapter). There is no live API
  ingestion, and Discussions and gists are out of scope.
- Inline review threads come from the REST `pulls/{n}/comments` endpoint, which
  does not expose resolved/outdated state. The body renders the thread location
  and comments, not whether the thread was resolved.
