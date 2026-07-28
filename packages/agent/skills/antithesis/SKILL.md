---
name: antithesis
description: >
  Cross-cutting facts and gotchas for any Antithesis work on the ix repo — credentials, harness
  structure, `setup_complete` protocol, assertion idioms, randomness rules, workload design,
  triage via `agent-browser`, signed report URLs, and Test Composer script placement. Loads
  whenever an Antithesis run, report, property/assertion, `snouty` invocation, or
  `antithesis_sdk::` call is in play. Complements the task-scoped `antithesis-{setup,workload,
  launch,triage,debug,query-logs,research,documentation}` skills.
---

# Antithesis (cross-cutting)

`snouty run` is never bare — go through the `antithesis-launch` skill. For a full setup walkthrough, `docs/antithesis.md`.

## Credentials & registry

- Webhook login: `bw://ix-infra/Antithesis Login/{username,password}`.
- GAR key (registry push): `bw://ix-infra/Antithesis gar key/notesPlain`.
- Registry: `us-central1-docker.pkg.dev/molten-verve-216720/indexable-repository`.
- Required env: `ANTITHESIS_TENANT=indexable`, `ANTITHESIS_USERNAME`, `ANTITHESIS_PASSWORD`, `ANTITHESIS_REPOSITORY`.

## Harness structure

Harness artifacts live at `nix/services/antithesis/default.nix`. One container, two binaries:

- `antithesis-harness` — long-running container command, emits `setup_complete`.
- `vcfs-workload` — dispatched by `argv[0]` as `serial_driver_*` / `finally_*`.

Test Composer scripts must land at `/opt/antithesis/test/v1/<test>/<prefix>_<name>` as real executables. Antithesis rejects images with fakeroot customisation layers as "Unsupported container type" — build with `dockerTools.buildLayeredImage` + `contents` only. No `extraCommands`, no `fakeRootCommands`.

## `setup_complete` protocol

1. Primary path: `antithesis_sdk::lifecycle::setup_complete(...)`.
2. Defence-in-depth: also append the JSONL record to `$ANTITHESIS_OUTPUT_DIR/sdk.jsonl` yourself.
3. The fallback MUST be non-fatal (log + continue). If the harness exits the container, Antithesis reports "No setup_complete received" and discards the run.
4. `snouty validate` requires `SNOUTY_CONTAINER_ENGINE=docker` when OrbStack is the docker runtime. (Podman rejects that env.)

`setup_complete` must come from the long-lived container entrypoint — never from a test command. Test commands don't run until after `setup_complete`, so emitting it from one deadlocks startup.

## Assertion idioms

- `assert_always!` — every-eval safety.
- `assert_sometimes!` — liveness / non-trivial-true-at-least-once.
- `assert_reachable!` — distinct outcome markers.
- `assert_unreachable!` — forbidden paths.

Never `assert_sometimes!(true, ...)` — that's `assert_reachable!`. SDK macros require literal messages; wrap in a workload-local macro if you need templating. Messages must be unique across call sites.

## Continue-on-fail

Failed assertions should log and keep driving the workload so Antithesis sees downstream behaviour. Early-exit on first failure hides cascades and wastes fuzzer budget.

## Randomness

All values driven by `antithesis_sdk::random::get_random()` or `random_choice(...)`. Never seed a local PRNG from SDK output — the branches would replay identical. For byte payloads, sample length with `get_random() % (MAX + 1)` and fill via repeated `get_random()`. `AntithesisRng` implements `rand::RngCore` if `rand` is already a dep; otherwise call `get_random()` directly.

## Workload design

1. Drive real SUT APIs.
2. Track expected state in-process.
3. Write an authoritative step log + projection as JSON summary.
4. `finally` driver re-opens state cold and verifies the projection.

Fixed-length values mask truncation bugs — use variable-length random byte arrays with proper truncate+write pairs, and read back exactly the expected length.

## POSIX write semantics (VCFS workloads)

Prefer truncate+write over write-at-offset when you need "this path now contains exactly these bytes". VCFS `write(offset=0, data)` is POSIX-like: overwrites `data.len()` bytes without shrinking. A shorter write after a longer one leaves stale tail bytes. That is correct POSIX, not a bug — asserting against stale tail bytes produces false positives that mask real bugs.

## Triage

Go through the `antithesis-triage` skill. It uses `agent-browser` (headless Chrome with SSO cookies) to read the React-rendered report. Raw `curl` fails because the report is GitHub-SSO gated. Report emails land at `indexable@reports.antithesis.com` with a signed URL — use that URL + `agent-browser`. The asset script at `.agents/skills/antithesis-triage/assets/antithesis-triage.js` exposes `window.__antithesisTriage.report.getAllProperties()` etc.

## Sharing report URLs

Antithesis reports are per-report-signed via `?auth=v2.public.<jwt>` on top of the tenant SSO. The bare `https://indexable.antithesis.com/report/<id>/<asset>.html` returns 403 for every visitor, even authed. **Never strip the `?auth=...` token when pasting** (Notion / Linear / Slack / docs). Paste the full URL as-is, plus a fallback pointer to `https://indexable.antithesis.com/runs` (find by run description) for when the signed token expires. Same rule for `logsUrl` from `getRecentRuns()` and example log URLs — treat the query string as opaque and required.

## Test Composer script placement

Scripts land at `/opt/antithesis/test/v1/<test_name>/<prefix>_<cmd>`. Prefix semantics:

- `first_` — once at start
- `serial_driver_` — sequential
- `parallel_driver_` — concurrent
- `singleton_driver_` — one-off
- `anytime_` — any point, including faults
- `eventually_` — faults paused
- `finally_` — end of timeline
- `helper_` — ignored by Antithesis

Test commands exit `0` on success.

## Related skills

- `antithesis-setup` — scaffold harness and compose file
- `antithesis-workload` — write assertions and test commands
- `antithesis-launch` — submit a run
- `antithesis-triage` — read a report
- `antithesis-debug` — multiverse debugger session
- `antithesis-query-logs` — cross-timeline log search
- `antithesis-research` — pre-setup codebase analysis
- `antithesis-documentation` — fetch the Antithesis docs pages
