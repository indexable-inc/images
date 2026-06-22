# source

`packages/source` is the workspace of source adapters that turn each data source
(a code checkout's neighbors: Slack, Linear, GitHub, git history, Claude/Codex
transcripts, shell history, journald) into embeddable, tagged search
[`Document`]s. One member, `source/meta` (crate `source-meta`), holds the shared
data model and traits every other member implements; the rest are the adapters.
The [`indexer`](../indexer/overview.md) drives them and the [sinks](../sink/overview.md)
consume them. All members are library crates (no flake outputs); they surface as
CI checks via `passthruTests`.

This page documents the shared `source-meta` model. The per-adapter grains,
ids, and tags are in [adapters.md](adapters.md).

## The Document model (`source/meta/src/lib.rs`)

[`Document`] (`meta/src/lib.rs:199`) is one record ready to upload: an
`external_id` (the store-stable id), a `file_name`, a static `mime`, the UTF-8
`body` to embed, a flat `meta_json` object, and a `content_hash`. Every record,
whatever its source, carries the common header [`DocumentMeta`]
(`meta/src/lib.rs:178`) flattened to top-level metadata keys, so each field is a
filter key: `source`, `external_id`, `content_hash`, `title`, optional `url`,
optional `timestamp`. The adapter merges source-specific extras into the same
flat object.

[`Source`] (`meta/src/lib.rs:50`) is the corpus tag: an open string newtype, not
an enum, so adding a corpus is a new tag value, never a match arm. `Source::code`
and `Source::web` name the two corpora the search pipeline treats specially
(local-manifest scoping, opt-in web results); every other tag is a generic
record source. [`KNOWN_SOURCE_TAGS`] (`meta/src/lib.rs:62`) is the list the query
edges validate user input against (a mistyped tag returns zero hits silently);
add an entry when an adapter gains a new `SOURCE_TAG`. [`RepoSlug`]
(`meta/src/lib.rs:151`) is the code-record repo identity (`Remote` from the git
origin, else `Local` from the directory name, never a silent empty string).

## Traits (`source/meta/src/lib.rs`)

- [`SourceAdapter`] (`meta/src/lib.rs:219`): turns one corpus into
  `documents() -> impl Iterator<Item = Result<Document, Error>>` plus a
  `source()` tag. Returning an iterator (not a `Vec`) lets a large export (a 344
  MB Slack tree) stream; a record that cannot be parsed is a typed `Err`, never
  silently dropped.
- [`Reconciler`] (`meta/src/lib.rs:251`): the consumer counterpart, implemented
  by the [sinks](../sink/overview.md). Its contract: desired state not deltas
  (`documents` is the source's complete current set), idempotent on
  `external_id` + `content_hash`, and source-scoped (a pass reads and writes only
  one source's records). A view can also satisfy idempotence at the storage
  engine level (e.g. a ClickHouse `ReplacingMergeTree`) instead of implementing
  the trait.

## Invariants enforced by construction

- **content_hash is the hash of the embedded bytes.** [`hash_body`]
  (`meta/src/lib.rs:271`) formats `sha256:<hex>` and is the only constructor, so
  a record's change-detection key can never drift from what was embedded.
  Re-ingesting an unchanged export is a no-op; a changed body re-embeds. Because
  the hash is over the sanitized bytes (below), the first re-sync after a sanitize
  change re-uploads the clean form.
- **Metadata stays in the store's limits.** [`check_metadata`]
  (`meta/src/lib.rs:327`) is a typed gate (`MAX_METADATA_BYTES` 128 KiB,
  `MAX_METADATA_KEYS` 256), so an over-budget record fails observably before
  upload rather than as an opaque 400 mid-ingest. It returns the serialized
  bytes so the caller does not serialize twice.

## Canonical keys (`source/meta/src/keys.rs`)

`keys` is the single source of truth for metadata key names, shared by adapters
(which write them) and the filter builder (which queries them), as `const`s so a
query can never target a key no adapter writes without the mismatch being visible
in one place (`keys.rs:1-6`). Groups: common (`SOURCE`, `CONTENT_HASH`, `TITLE`,
`EXTERNAL_ID`, `URL`, `TIMESTAMP`), code (`REPO`, `PATH`), git commits (`COMMIT`,
`AUTHOR_NAME`, `AUTHOR_EMAIL`), Slack (`CHANNEL_ID`/`CHANNEL_NAME`, `AUTHORS`,
`IS_EXTERNAL`, `IS_BOT_THREAD`), agent history (`HOST`, `USER`, `PROJECT`,
`SESSION_ID`, `MESSAGE_UUID`, `PARENT_UUID`, `ROLE`, `RECORD_TYPE`, `MODEL`,
`CWD`, `GIT_BRANCH`, `TOOL_NAME`, token counts), shell (`EXIT_STATUS`), journald
(`UNIT`), Linear (`IDENTIFIER`, `TEAM_KEY`, `STATE_TYPE`, `ASSIGNEE_EMAIL`,
`LABELS`, `IS_ARCHIVED`), and GitHub (`NUMBER`, `STATE`, `IS_PR`, `KIND`,
`WORKFLOW`, `BRANCH`, `CONCLUSION`, `RUN_NUMBER`).

## Body sanitization (`source/meta/src/sanitize.rs`)

One shared pipeline every adapter applies before a body is hashed and embedded
(`sanitize.rs:1-27`). Three concerns: strip terminal ANSI/OSC noise; redact
secrets (a conservative, prefixed pattern table mapping a credential shape to
`[redacted:<kind>]`, e.g. GitHub/Linear tokens found verbatim in past indexed
transcripts); and collapse base64/hex blobs longer than `BLOB_TOKEN_CHARS` (120)
to a `[blob NNN chars]` marker. `sanitize_tool_result` additionally caps one
tool-result section at `TOOL_RESULT_CAP_CHARS` (4000, head+tail around a
truncation marker) so a huge CI log cannot dominate the document it folds into.
Sanitizing before hashing means a re-sync sees previously raw bodies as changed
and replaces them with the clean form.

## Adapters

The 10 source adapters plus the `source-parquet` log reader are tabulated in
[adapters.md](adapters.md): each one's grain, `external_id` shape, and the filter
tags it writes.

[`Document`]: #the-document-model-sourcemetasrclibrs
[`DocumentMeta`]: #the-document-model-sourcemetasrclibrs
[`Source`]: #the-document-model-sourcemetasrclibrs
[`KNOWN_SOURCE_TAGS`]: #the-document-model-sourcemetasrclibrs
[`RepoSlug`]: #the-document-model-sourcemetasrclibrs
[`SourceAdapter`]: #traits-sourcemetasrclibrs
[`Reconciler`]: #traits-sourcemetasrclibrs
[`hash_body`]: #invariants-enforced-by-construction
[`check_metadata`]: #invariants-enforced-by-construction
