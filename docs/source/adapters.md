# source adapters

The concrete [`SourceAdapter`](overview.md) implementations under
`packages/source/*`, plus the `source-parquet` log reader. Each turns one corpus
into [`Document`](overview.md)s carrying the common envelope (`source`,
`external_id`, `content_hash`, `title`, `timestamp`) merged with the
source-specific filter tags below. All are library crates; the
[`indexer`](../indexer/overview.md) selects which to run.

## Adapter table

| crate | `source` tag | grain (one Document per ...) | `external_id` | source-specific tags |
| --- | --- | --- | --- | --- |
| `source/atuin` | `shell` | recorded shell command (nushell/zsh/bash, one atuin db) | `atuin:{id}` | `host`, `user`, `cwd`, `session_id`, `exit_status` |
| `source/claude` | `claude_history` | transcript message (tool result folded into its tool_use) | `claude:{session_id}:{uuid}` | `host`, `user`, `project`, `session_id`, `message_uuid`, `parent_uuid`, `role`, `record_type`, `model`, `cwd`, `git_branch`, `tool_name`, token counts |
| `source/codex` | `codex` | submitted prompt, and each session-rollout item | prompt `codex:{session_id}:{ts}:{content_hash}`; rollout `codex:{session_id}:{content_hash}` | `host`, `user`, `session_id`; rollout adds `role`, `record_type`, `model`, `cwd`/`project`, `tool_name` |
| `source/debug` | `claude_debug` | session `--debug` log file | `claude_debug:{session_id}` | `host`, `user`, `session_id` |
| `source/git` | `git` | commit (message only; diff stays in git) | `git:{repo}:{sha}` | `repo`, `commit`, `author_name`, `author_email` |
| `source/github` | `github` | issue, pull request, and failed CI run | item `github:{owner}/{repo}:{number}`; CI `github:ci:{owner}/{repo}:{run_id}` | `repo`, `number`, `state`, `is_pr`, `labels`; CI adds `kind=ci_run`, `workflow`, `branch`, `conclusion`, `run_number`, `commit` |
| `source/journald` | `journald` | (unit, UTC day) slice of priority<=4 messages | `journald:{host}:{unit}:{date}` | `host`, `unit` |
| `source/linear` | `linear` | issue (description + comments, oldest first) | `linear:issue:{id}` | `identifier`, `team_key`, `state_type`, `assignee_email`, `labels`, `is_archived`, `url` |
| `source/slack` | `slack` | thread (every message sharing `channel_id`+`thread_ts`) | `slack:{channel_id}:{thread_ts}` | `channel_id`, `channel_name`, `authors`, `is_archived`, `is_external`, `is_bot_thread`, `message_count`, `has_files`, `thread_ts` |

Each crate exports its tag as a `SOURCE_TAG` const (e.g. `atuin/src/lib.rs:35`,
`claude/src/lib.rs:39`, `codex/src/lib.rs:56`, `debug/src/lib.rs:40`,
`git/src/lib.rs:32`, `journald/src/lib.rs:46`); github/linear write `"github"` /
`"linear"` inline. When a new tag appears, add it to
[`KNOWN_SOURCE_TAGS`](overview.md) so the query edges accept it.

## Reading shapes

- **History (per host/user): atuin, claude, codex, debug.** Read live local
  files (an atuin SQLite db opened `immutable=1` so a live shell is never
  blocked, `atuin/src/lib.rs:75`; `*.jsonl` transcripts; the codex prompt log
  plus session rollouts; `--debug` text logs). Append-only, so an unchanged file
  re-ingests nothing under the content-hash gate, and a live file (the current
  codex session, today's debug log) re-uploads only while it grows. The
  privileged fleet run reads other accounts' homes as root, so these adapters
  index only regular files and the caller does a symlink-safe path check, and
  index only regular files to refuse a planted symlink (`debug/src/lib.rs:17-22`).
  These are never `--gc`'d: their scans are incremental windows, not complete
  snapshots.
- **Bulk exports (export-complete): github, linear, slack.** Read a directory of
  JSON an exporter wrote. github/linear are pure readers (the joins live in the
  exporter; `github/src/lib.rs:1`, `linear/src/lib.rs:1`); slack streams channel
  by channel so a 344 MB tree is never fully in memory (`slack/src/lib.rs:13-19`).
  Their complete input makes them eligible for the indexer's `--gc` pass.
- **git, journald** shell out once (`git log` with a US/NUL pretty format,
  `git/src/lib.rs:34`; `journalctl -o json --priority 4 --since`,
  `journald/src/lib.rs:22`) and project the parsed output. git is export-complete
  (GC-eligible); journald is a windowed read.

All transcript and log adapters route message text through the shared
[`source_meta::sanitize`](overview.md) pipeline (ANSI stripped, credential
shapes redacted, blobs collapsed, tool sections capped) before hashing and
embedding.

## source-parquet (the log reader)

`source/parquet` (crate `source-parquet`) is the consumer half of the parquet
corpus log written by [`sink-parquet`](../sink/overview.md), not an adapter. It
lists an S3/R2 prefix, reads each `<prefix>/source=<source>/data.parquet`, and
reconstructs [`Document`](overview.md)s from four columns (`external_id`,
`content_hash`, `body`, `meta_json`); the rest of the schema is a projection out
of `meta_json` and ignored on read (`parquet/src/lib.rs:14-22`). The
`_manifest.json` sidecar and any non-`/data.parquet` object are skipped. This is
the materialized-view-over-a-log model: the parquet log is the append-only truth
and the Mixedbread index is one view replayed from it (issue #736). The indexer's
`--from-parquet-prefix` consume mode uses it. Known limitation: it lists the
whole prefix and materializes every row each run (`parquet/src/lib.rs:23`); the
incremental cursor lives in the [Iceberg lake](../lake/overview.md) instead.
