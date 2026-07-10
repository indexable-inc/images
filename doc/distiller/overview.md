# distiller

`packages/distiller` (`ix-distiller`) distills ReasoningBank-style lessons from
local Claude Code transcripts into three artifacts: human-readable facts markdown
per `(user, project)`, a `source=distilled_facts` corpus parquet slice (the
lessons), and a `source=session_outcomes` slice with one LLM-judged outcome
verdict per session. The slices ride the existing archive -> Iceberg lake ->
Mixedbread funnel with zero Rust changes: the leader fold ingests any
`(host, user, source)` slice generically (`README.md`, `default.nix:1-5`).

It is a pure-Python package (not a Rust workspace member). Entry point
`ix-distiller` runs `python -m distiller` (`default.nix:55-59`).

```
ix-distiller --days 7 --user andrew --out /var/lib/ix-distiller \
  [--project index] [--model claude-haiku-4-5-20251001] \
  [--upload [--env-file /run/ix-secret-store/env/ix-indexer]]
```

## Pipeline

The CLI (`src/distiller/cli.py`) drives five stages:

1. **Scan** (`transcripts.scan`, `transcripts.py:213-243`). Parse every
   `~/.claude/projects/*/*.jsonl` newer than the `--days` window (mtime gate plus
   a message-timestamp gate), skipping `subagents/` transcripts. Each file
   becomes a `Session` (`transcripts.py:37-58`) with extracted signals: the goal
   (first real user message), user corrections (regex `_CORRECTION_RE`,
   `:21-26`), tool errors (`is_error` tool_results), success markers
   (`_SUCCESS_RE` including "Pushed to main", `:30-34`), the final assistant
   message, models used, and a noisy heuristic `outcome` label. Unparseable
   lines are skipped, mirroring the Rust adapter `packages/search/source/claude`.
2. **Group and filter** (`cli.run`, `cli.py:65-87`). Group sessions by project
   (`cwd`), drop the distiller's own headless sessions via the `PROMPT_SENTINEL`
   guard (`distill.py:25`), apply `--project` substring filters, and skip
   projects under `--min-sessions`.
3. **Distill + judge** (`distill.run_claude`, `distill.py:150-207`). One headless
   `claude -p` call per project (default model `claude-haiku-4-5-20251001`,
   `distill.py:20`), run from a throwaway cwd with `--strict-mcp-config
   --mcp-config '{"mcpServers":{}}' --max-turns 2 --output-format json`. The
   prompt (`distill.py:27-66`) carries the existing playbook items plus new
   session digests and asks for itemized `add`/`update` operations only (never a
   full rewrite) AND exactly one per-session verdict
   (`success`/`partial`/`failure`/`abandoned`). The envelope parser tolerates
   both old `{"result": ...}` and current event-array shapes (`distill.py:118-134`).
4. **Merge** (`distill.apply_operations`, `distill.py:261-343`). Apply operations
   into the existing item list with stable ids (`df-<hex>`,
   `distill.py:257-258`): `add` (capped at `--max-new-items`, near-duplicate
   title guard), `update` by id, and items not mentioned survive verbatim. This
   is the anti-collapse invariant (ACE: incremental itemized updates, never
   wholesale regeneration). `session_verdicts` (`distill.py:217-254`) normalizes
   model verdicts to the closed `SESSION_LABELS` set and falls back to scan
   heuristics for any unjudged session so the verdict set has no holes.
5. **Write + validate + upload** (`cli.py:157-207`). Build rows, write each slice,
   re-validate, and optionally upload.

State (items, seen sessions, verdicts) is per `(user, project)` under
`<out>/state/<user>/<slug>.json`, written atomically (`state.py`). A session is
re-distilled only when its `fingerprint` (message count + last timestamp) changed
(`cli.py:103-107`, `transcripts.py:56-58`), which is what makes runs incremental.

## The corpus contract (`corpus.py`)

Both slices use the exact 9-column schema mirroring `packages/search/sink/parquet`
(`corpus.py:38-50`): `external_id, source, content_hash, title, url, host,
timestamp(int64), body, meta_json`, all Utf8 but `timestamp`, with
`external_id/source/content_hash/body/meta_json` non-null. Load-bearing details:

- `content_hash` is exactly `sha256:<hex>` of the body bytes (`hash_body`,
  `corpus.py:59-60`).
- `_manifest.json` carries `content_hash` = sha256 over the sorted set of
  `(external_id NUL content_hash NUL)` pairs (`corpus_hash`, `corpus.py:63-75`),
  byte-for-byte the sink's construction.
- `meta_json` is a flat object (<=128 KiB, <=256 keys) carrying the standard
  filter keys (`user`/`host`/`project`/`timestamp`/`scope`) plus, for lessons,
  `session_labels` and `failure_derived` when evidence includes a failed session
  (`item_row`, `corpus.py:96-151`).
- Files are written with pyarrow so the embedded Arrow schema says `Utf8` (what
  the Rust source-parquet reader downcasts to); an empty row set writes nothing,
  never a wipe (`write_slice`, `corpus.py:230-249`).

`validate_slice` (`corpus.py:252-326`) re-reads each slice with polars (a second
parquet implementation so pyarrow cannot self-certify), and asserts column
order/dtypes, non-null contract, per-row body hashes, `meta_json` identity and
limits, the manifest hash, and the physical Utf8 arrow types.

Lessons also render to `<out>/facts/<user>/<slug>.md` (`markdown.py`). The
`session_outcomes` slice carries one row per judged session: body = reason + key
stats, `meta_json` = label, turns, duration, models (`session_row`,
`corpus.py:191-227`).

## Upload (`upload.py`)

`--upload` puts `data.parquet` + `_manifest.json` under
`corpus/host=<h>/user=<u>/source=<src>/` into the MinIO archive (default endpoint
`http://127.0.0.1:9010`, bucket `ix-history`, prefix `corpus`) via boto3
(`upload_slice`, `upload.py:42-66`). Credentials come from the environment
(`AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY`, or `MINIO_ROOT_*`) or an
`--env-file` in systemd EnvironmentFile format. The leader's hourly fold + view
reconcile then make the rows searchable via
`search.semantic(..., source=["distilled_facts"])` (or `["session_outcomes"]`).
Updates fold each `external_id` to its newest `content_hash`; vanished ids are not
deleted (append/merge fold, ENG-2696), so deletion needs an explicit tombstone.

## How it is built and tested

`default.nix` copies the pure-Python source into a pinned interpreter via
`toPythonModule`, wraps `python -m distiller`, and runs two sandbox passthru
tests: `pytest` over `tests/` and an import smoke test that also parses the CLI
(`default.nix:73-110`). Runtime deps are polars (validation re-reader), pyarrow
(writer), and boto3 (`--upload`) (`default.nix:34-43`). Flake output `distiller`
(`package.nix`):

```
nix build .#distiller            # import smoke test
```

The `tests/` suite covers the parquet contract (schema, content/manifest hashes,
tamper rejection), the incremental merge (stable ids, update-not-rewrite, caps),
transcript signal extraction, and the outcome-labeling path (verdict
normalization + fallback, the `session_outcomes` slice, failure-derived lesson
marking, and an end-to-end run against a mocked `claude -p`).

## Relationship to other agents tooling

The slices distiller publishes are exactly what the
[claude-hooks](../claude-hooks/overview.md) `prompt-priors` hook later surfaces
back into new sessions (it searches `claude_history,shell,github` via
`IX_SEARCH`; `distilled_facts`/`session_outcomes` become additional searchable
sources). distiller closes the transcript -> lesson -> recall loop.
