# symphony

An Elixir runtime that orchestrates Codex agent sessions across one or
more git repositories. Workflows are written in the `.sym` surface
language, lowered to an IR run graph the runtime walks; hot-reloaded
`.sym` workflows and markdown skills are the configuration surface. The
Rust workspace at `packages/` ships the room backend (`room-server`) and
a Tauri desktop client (`packages/room`).

Do not commit secrets. Tokens for Linear, GitHub, Slack, Codex, or any other
external system must be supplied through the runtime environment or host
secret manager. The bundled `.env.example` lists the keys the runtime reads.

Commits must not include personal or assistant attribution trailers.

## Scope of AGENTS.md

`AGENTS.md` is for durable working principles. Add guidance here only when it
applies to a class of future changes across the repo, or when it captures an
architecture invariant that would be expensive to rediscover.

The test for a new rule is generality. It should survive the specific feature
that prompted it, apply to the next helper or module with the same shape, and
read more like a design philosophy than a task note. Specific examples are
fine when they sharpen the rule. The example should never be the rule.

Put local facts in the narrowest home that owns them: README files, option
descriptions, generated reference, issue bodies, module docs, or an inline
comment next to the load-bearing line. When a narrow note keeps growing
across features, promote the broad invariant here and leave the local
details where operators will look first.

Before adding durable guidance, search the tree and existing docs first.
Facts that are easy to rediscover with source search, generated reference, PR
history, or a narrow README should stay out of this file.

Each addition should be one or two direct sentences. Name the invariant,
owner, or decision rule, and include a path, command, URL, or external
reference only when it is the durable handle for that rule.

## Self-contained operations

Symphony's runtime behavior must not depend on out-of-repo changes to
function. In particular, scheduled work (cron triggers, dispatchers,
auto-healing loops) belongs inside the runtime, driven by Symphony's own
cron scheduler. Do not introduce systemd timers, host nix modules, or any
out-of-repo schedulers as load-bearing pieces of a symphony feature. A fresh
symphony deploy should bring up all of its scheduled work without needing a
paired change in any other repo.

## Workflow packs

The runtime is pack-agnostic. The bundled `workflows/example/` pack is the
public default and is intentionally narrow (a single manual-trigger inspect
skill). Deployers point `SYMPHONY_PACK_DIR` at their own pack to drive real
work. Keep core changes pack-agnostic: no workflow names, repo slugs,
label strings, or ticket schemes hardcoded in `elixir/lib/`.

## Workflow

Pull `main` before starting. Make changes on a short-lived branch. Use the
`codex/` branch prefix unless the user asks for a different name.

After local checks pass, push the branch and open a PR targeting `main`.
Enable auto-merge as soon as required checks and review state allow it.
Watch required checks with `gh pr checks --watch --fail-fast`; if a check
fails, inspect the run logs, fix the branch, push again, and restart the
watcher. Keep that loop going until GitHub reports the PR merged or a human
explicitly asks you to stop.

Treat PR comments and reviews as part of the work. Read them with
`gh pr view --comments` and the review fields from
`gh pr view --json reviews`. Address AI review comments in code when they
identify a real issue, reply when a comment is intentionally declined, and
resolve review threads before relying on auto-merge.

Check the PR author before pushing to, closing, merging, enabling
auto-merge for, or otherwise modifying a PR. Do not change PRs authored by
another GitHub user unless that user or the operator explicitly authorizes
it.

Delete the local branch after the PR has merged.

Commit one logical change at a time. Use the pathspec form so unrelated
staged or unstaged files cannot ride along:

```sh
git commit -m "scope: imperative subject" -- <paths>
```

Subjects are imperative, lowercased, and have no trailing period. The
optional scope names the layer being touched, such as `room-server:`,
`elixir:`, `flake:`, or `AGENTS:`. Use a body only for the reason the diff
cannot show. If a commit fixes a tracked GitHub issue, include
`Fixes #123`, `Closes #123`, or `Resolves #123` in the body. Use
`Refs #123` for related or partial work.

`main` is the long-lived human branch. PRs target `main`. Deployment refs
are tags on commits that are already reachable from `main`.

## Writing style

These rules apply to prose in docs, READMEs, comments, issues, and PR
descriptions.

Start with the reader's task. A README opens with a short plain-language
summary directly under the title, then moves into task-specific headings.
Keep paragraphs short. Remove completeness theater.

Write in concrete nouns. Link the first mention of repo-owned tools,
packages, commands, directories, and important upstream projects in each
section. Match upstream capitalization: `nixpkgs`, `systemd`, `Elixir`,
`Tauri`, `Loro`.

Use measured details where they matter. A number, command, file path,
upstream issue, or failure message earns more trust than a smooth
adjective. Prefer "the broadcast loop fires at 12.5 Hz" over "fast".

Name limits and failure modes. A short "bad fit if" or "known limitations"
paragraph often helps more than another claim of strength. Say what
breaks, how to notice it, and which workaround hurts.

Avoid slogan shapes that contrast a good phrase with a bad one, such as
`X, not Y` or `X, don't Y`. State the desired thing directly. Avoid em
dashes; split the sentence or use a colon.

## Inline comments

Comments explain why a line exists, which failure it prevents, or which
external constraint pins the choice. They should add information the
syntax cannot recover. Delete comments that narrate the code.

Leave a comment when something looks redundant but a build, eval, or test
proves it is load-bearing. Put the observed symptom next to the line that
survives the obvious cleanup.

Non-obvious technical decisions need a public reference when one exists:
RFC, JEP, upstream issue, vendor doc, benchmark, errata, or design note.
Put the URL in the comment near the choice. If no public reference
exists, say where the decision came from.

## Rust style

Repo-owned crates, fixtures, examples, and generated manifests use Rust
edition 2024. Fix compatibility issues directly and document unavoidable
upstream blockers next to the exception.

Prefer names that preserve the concept's path. Local aliases may shorten
noisy source paths only when the shape remains visible at the call site.
Keep singular names for single values and plural names for bags of
constructors, helpers, or registry entries.

Use local type annotations when they make the data shape clearer. Keep
turbofish for expression-local cases where an intermediate binding would
add noise.

Use normal module layout. Move files so `mod` declarations follow the
filesystem instead of using `#[path = ...]`.

Avoid anonymous tuple-shaped domain data once a value crosses a function
boundary. Prefer named structs or full paths for values that carry real
meaning.

Use blank lines as paragraph breaks inside functions: set up, act, then
validate or return. Keep tightly coupled statements together.

When parsing, normalizing, serializing, traversing graphs, handling
archives, or speaking protocols, start from a maintained crate.
Hand-written logic is for the thin glue around that crate unless the
dependency boundary is measurably worse.

## Elixir style

The Elixir runtime is the entry point for symphony itself; the Rust
crates and the Tauri client are subsystems it does not own. Keep
`elixir/lib/` pack-agnostic, with workflow shape carried in `.sym` /
markdown under the active pack directory rather than hardcoded in source.

Prefer Mix tasks and supervised processes over loose scripts. A new
scheduled job is a child of Symphony's cron supervisor, not a host-level
timer.

## Engine host

The room-server is the model-agnostic engine host: every engine
implements the one `Engine` trait in `packages/room-server/src/engine.rs`
and emits the canonical `EngineEvent` union, so `bridge`, `state`, and
`http` dispatch on the request's `engine` field and never name a
concrete engine. A new engine is a new adapter plus an `EngineHandle`
variant, not a new branch in the consumers.

## Room transport: WebTransport over HTTP/3

The room server speaks WebTransport (HTTP/3 over QUIC) to peers, not
WebSocket. Reliable bidirectional streams carry the Loro sync framing
(JSON text deltas, binary CRDT updates, periodic pings); unreliable
datagrams carry Opus audio for the room's shared voice channel. The
server is a dumb SFU for audio: it never decodes, it just routes a
datagram to every other peer in the room.

WebTransport is Baseline as of Safari 26.4 (March 2026), so the Tauri
WKWebView gets the API on macOS 26.4 and later without a polyfill or a
Rust-side workaround. Older macOS WKWebViews do not have WebTransport
and are not supported.

Server-side use `wtransport` (`https://docs.rs/wtransport`). It wraps
`quinn` for QUIC, supports datagrams and bidi streams in one
connection, and accepts a self-signed cert at boot for dev. The
SHA-256 fingerprint of that cert is served over the existing axum
HTTP listener so clients can pin it via the WebTransport
`serverCertificateHashes` constructor option (the same path Chromium
documents at `https://developer.chrome.com/docs/capabilities/web-apis/webtransport`).
Browser-side hash-pinned certs must have validity under 14 days, so
the dev cert is regenerated at every server start.

Audio: capture with `getUserMedia` then chop into 20 ms PCM frames
inside an `AudioWorklet`, encode with the WebCodecs `AudioEncoder`
configured for `codec: "opus"`, `bitrate: 16000`,
`bitrateMode: "constant"`, and ship each Opus packet as one
datagram. Playback decodes through `AudioDecoder` and schedules PCM
through a sibling `AudioWorklet` jitter buffer. Drop late frames; do
not retransmit.

## Room client patterns

The Tauri desktop client at `packages/room` is a Svelte 5 app sharing a
`LoroDoc` across peers for ephemeral state. The runtime contracts that
took iterations to land:

Use the Loro primitive that matches the shape of the state. A
`LoroText` is the right home for any shared text: both peers get a
character-level CRDT, and `LoroText.update(next)` diffs against current
state and emits the minimal insert/delete ops. Broadcasting full text
snapshots through a `LoroMap` field and reconciling locally is the
wrong shape. Concurrent edits resolve as last-write-wins, and the
client ends up writing baseline-divergence heuristics that approximate
merge semantics rather than implementing them. The same applies to
peer cursors: `LoroText.getCursor(pos)` returns a stable anchor that
survives concurrent edits, and `doc.getCursorPos(cursor)` resolves it
back to a live index. An integer offset is a pragmatic fallback only
when the drift window is sub-frame and the wire encoding cost is real
(JSON presence, base64 round-trip).

Wrap the Loro write surface so consumers do not call commit and flush.
`roomDoc` exposes thin helpers (`setSelf`, `composerText`) that own
`doc.commit()` and the outgoing-frame flush. Components never reach
into the `LoroDoc` directly. Cache one wrapper per logical entity (one
`ComposerText` per thread id) so repeat calls share a single
subscription.

For ephemeral state with TTL eviction (cursors, selections, focus),
the upstream-idiomatic primitive is `EphemeralStore` rather than
a `LoroMap` of JSON blobs. The current `presence` map predates the
realization and stays for now; new ephemeral surfaces should reach for
`EphemeralStore`.

Throttle outgoing Loro writes with a trailing window. A timer that
resets on every event will starve under a continuous source (assistant
streaming, fast typing), because each new event pushes the deadline
forward and the write never fires. Use a schedule-once timer that
fires at the trailing edge regardless of incoming events. 80 to 100 ms
is the sweet spot for human-perceptible updates without flooding the
wire.

Optimistic local writes commit in the same frame as user intent.
Press Enter, the textarea clears, the Loro state updates, the
transcript shows the message, and the HTTP POST runs in the background
as pure persistence. Tag optimistic identifiers with a sentinel prefix
(`local-`) so dedup-on-confirm by content is unambiguous; the dedup
branch lives next to the real insert path so the server-confirmed copy
replaces the optimistic without flicker. On failure, restore the input
only if the user has not started typing the next message. Replacing
fresh local intent with a retry is worse than losing the failed write.

The CSS Custom Highlight API requires live `Range` objects. Do not
cache `Range` instances across rebuilds. `MarkdownBody` renders with
`{@html ...}`, so any reactive update detaches the underlying text
nodes; cached Ranges then reference detached nodes that WebKit may
keep painting at the old position until the next compositor pass.
Recompute the Range set on every rebuild, and clear the registry
eagerly when the input changes so the throttle window cannot keep
stale paint visible.

Children keyed by a parent prop are not reactive on that prop. When
`ThreadDetail` wraps a child in `{#key threadId}`, the child is
remounted on every switch and once-at-mount reads are correct. Wrap
those reads with `untrack(() => ...)` to silence Svelte 5's
prop-tracking warning. This applies to `roomDoc.composerText(id)`,
`getDraft(id)`, and similar id-keyed lookups.

Mirror Loro state into Svelte with one effect per subscription.
`$effect(() => store.subscribe(set))` mirrors a Loro-backed readable
into component-local `$state`, and the returned unsubscribe is the
effect cleanup. Do not gate writes on focus or component lifecycle,
which starves initial sync on remount.

Do not read mirrored `$state` inside the subscriber callback. Svelte
stores fire the listener synchronously on attach to deliver the
initial value, and that synchronous fire lands inside the
`$effect` body. Any `$state` read there registers the variable as a
dep of the effect, the effect re-runs on every write to that
variable, the re-subscription delivers the readable's stale value,
and the callback clobbers the just-written local value. Assign
unconditionally instead and rely on `$state`'s identical-value
no-op to terminate the local-write cycle. If a guard against
duplicate work really is needed, store the last-seen value in a
plain local variable, not in `$state`.

## Sane defaults

Helpers, modules, packages, templates, examples, and generated commands
should be useful in the common production-shaped path without extra
ceremony. Defaults should be checked, typed, reproducible, conservative
about secrets and networking, and easy to override with a named reason.

Prefer the future-correct interface over compatibility layers. This repo
can change its own callers when an old spelling makes the safe path
harder to express. Remove migration branches and stale aliases in the
same change that introduces the clearer interface unless the user
explicitly asks for a migration window.

For a small choice, lead with the direct answer and the shortest working
path. Save comparison tables for long-lived boundaries or vendor
commitments.

Before finishing a change, reread the diff with suspicion. Ask whether
the owner is clear, whether a helper or type would remove real
duplication, whether a boundary is string-shaped when it should be
typed, and whether a smaller API would make the next change easier.

Fix root causes at the owner. When the same adapter, default,
conversion, or fallback appears in multiple places, move the capability
transition or invariant to the boundary that owns it.

Turn assumptions into checked behavior through types, schemas, module
options, derivation checks, or focused tests. If the user asked for a
fix, land code and the nearest durable test or validation hook;
diagnosis alone is unfinished work.

Delete vestigial code in the same change that makes it obsolete. Dead
fields, options, structs, functions, configs, generated files, and
compatibility shims make the safe path harder to see.

When adding a non-obvious workaround, policy exception, or operational
guard, put the reason near the choice and cite a durable source when
one exists.

## User-facing commands

Keep protocol emitters separate from product workflow code. Workflows
should produce facts; terminal, API, and document surfaces should render
those facts for their audience.

Human-readable output is the default for interactive commands. Agents,
scripts, and tests should prefer machine-readable output when the
command supports it.

Long-running commands should expose the phases users naturally ask
about. Terminal progress should keep moving while work is in flight,
with recent rate and cumulative totals reported as separate facts when
both matter.

Default errors should lead with the operator-facing failure and
actionable context. Source locations, backtraces, trace paths, and
internal module paths belong behind debug output or structured output
unless they are the user's next step.

## Nix philosophy

`flake.nix` is the manifest. It should expose inputs and delegate
outputs to package-specific files. Keep scenario wiring, artifact
manifests, app wrappers, and helper logic near the owner that changes
with them.

A workflow command is a `packages.<name>` derivation with
`meta.mainProgram` set so `nix run .#<name>` and `nix build .#<name>`
point at the same derivation. Do not add a parallel `apps.<name>` entry
that re-wraps a package; that doubles the surface and silently drifts
the moment the binary path changes. Use the `apps` output only when the
runnable program legitimately has no derivation of its own.

Use standard flake outputs: `packages`, `checks`, `formatter`,
`devShells`, `templates`, `overlays`, `nixosModules`, and `lib`.

Expose aggregate knowledge as data before wrapping it in a command. A
`lib.<name>` value that `nix eval --json` can inspect is easier to
reuse than a one-off app. Add a wrapper when formatting, joins,
follow-up actions, or human-facing output justify it.

Prefer one source of truth. Discovery beats hand-maintained registries.
Generated catalogs should come from small manifests. Hashes live with
URLs. Versions live near the package or ecosystem that owns them.

Keep eval pure. Inputs flow through `flake.nix` or typed parameters.
Avoid host environment reads, channel refs, ad hoc flake paths, and
eval-time network fetches.

Generate commands through checked helpers. A wrapper reached through
`nix run .#...` should call realized executables with `lib.getExe`, an
app program, or an explicit store path reference. Avoid nesting another
flake frontend inside the generated command.

Nix builders for language workspaces should pass the smallest source
closure the compiler can consume. The caller names both the filtered
`src` and the real `workspaceRoot`; do not infer one from the other.

Nix source filtering and flakes only see tracked or staged source
files. Stage new source files before running Nix validation so failures
describe the expression under test instead of a missing path.

The Tauri client at `packages/room` is not Nix-built. Building Tauri
under Nix (WebKit, codesign, bundle formats) is a separate project this
repo has not taken on. Build it locally with `npm run tauri:dev` or
`npm run tauri:build` from `packages/room`.

## Module conventions

Modules declare options and config. Keep each module inert until its
enable flag or equivalent activation condition is set. Prefer
independent modules over modules importing each other.

A new module is a directory at `modules/<category>/<name>/` with its
own `default.nix`. Helper data that lives next to a module but is not
itself a NixOS module belongs in a sibling directory whose name starts
with `_`.

Public options should describe the user's domain. Hide storage
mechanics behind typed options, generated files, and small adapters.
Use broad escape hatches only at true foreign-format boundaries and
name that boundary in the description.

Structured config belongs in structured values. Prefer
`pkgs.formats.*`, freeform submodules, and typed option trees over
string fragments that cannot merge, inspect, or receive `mkDefault`
and `mkForce` cleanly.

Every module that binds a TCP or UDP socket should declare a port
claim next to the bind setting or firewall declaration. A duplicate
claim in the same namespace is a useful eval-time failure; intentional
co-location needs a real namespace boundary or an explicit alternate
port.

## Dependency intake

Every external input needs an owner that can update it predictably.
Prefer ecosystem lockfiles, flake inputs for real flake-level tools,
repo manifests consumed by updaters, or narrow `pkgs.*` fetchers when
no better owner exists.

The human workflow is: edit the source requirement or manifest, run the
owning updater, inspect the generated diff, and commit the source and
generated hash-bearing output together.

Use the most specific `pkgs.*` fetcher for the source: `fetchurl` for
opaque single files, forge fetchers for forge snapshots, `fetchgit`
for raw git refs, `fetchzip` for archives that must unpack, and
ecosystem fetchers when one exists. Avoid `builtins.fetch*` in
tracked Nix files because those fetch during eval and do not
substitute like fixed-output derivations.

Tracked Nix files should never contain fake hash helpers or
placeholder hashes. Materialize real SRI hashes with the owning
updater, lock command, `nix flake update`, or a checked prefetch
command before committing.

Generated catalogs are build inputs, not hand-edited source. If a
generated file is wrong, change the manifest or generator that owns
it.

## Nix practices to tighten

Improve these patterns when touching nearby code. If cleanup is wider
than the task, file a narrow issue.

- Prefer precise option types over broad attrs. Keep broad attrs at
  true foreign format boundaries.
- Filter local sources to the smallest useful tracked file set.
- Use `lib.getExe` or `lib.getExe'` instead of spelling
  `${pkg}/bin/foo` repeatedly.
- Use `pkgs.stdenv.hostPlatform.system` instead of the deprecated
  `pkgs.system`.
- Do not add `apps.<name>` entries for programs that already have a
  package with `meta.mainProgram`.
- Keep validation in shared builders and reuse those builders
  everywhere.
- Default to no `devShells.default`; add per-package shells or build
  inputs where the need belongs.

## Nix style

The common hard rules are:

- No `with pkgs;` or `with lib;`. Use `inherit (pkgs) ...` or
  `lib.foo` directly.
- No `rec { }`. Use `let ... in` or `final` / `prev`.
- No `mkForce`. Fix the module boundary or compose priorities
  deliberately.
- No `lib.recursiveUpdate`. Build the attrset in one place or use
  `lib.mkMerge`.
- No repeated parent keys in the same attrset. Group related
  assignments under one parent.
- Prefer `inherit (source) name;` for direct same-name field copies.
- No `builtins.currentSystem`, `builtins.getEnv`, `<nixpkgs>`, or
  `path:` flake refs.
- No `(import ./foo.nix)` inside `imports = [ ... ]`; NixOS
  auto-imports paths.
- No bare `assert cond;`. Use an assertion that names the failure.
- No unused bindings. Use `_` for intentionally unused lambda
  arguments.
- Set `strictDeps = true` on every `mkDerivation`.
- Use `pkgs.*` fetchers instead of `builtins.fetch*`.
- Commit real hashes, never fake hash helpers or placeholders.
- Use `nixosModules.<name>` for module exports. Avoid a flat
  top-level `modules` output.

## Issues

Keep issue bodies short: problem, context, desired outcome. Bug
reports need a concrete reproduction command or step list. Avoid
prescribing implementation unless that is the actual request.

When creating or editing GitHub issue bodies or comments, pass
multiline text through a real multiline input path such as
`--body-file -`, a temporary file, or an editor. Escaped `\n`
sequences in quoted `--body` strings render literally on GitHub.

Prefer GitHub's suggestion block syntax for proposed inline changes
in PR review comments on changed lines. Use fenced `suggestion`
blocks only when GitHub can apply the snippet directly.

When work exposes a real bug, broken assumption, or unidiomatic
pattern that will outlive the current task, file a GitHub issue
right then. One concrete observation per issue.

## Tests

Tests should protect behavior that can regress across boundaries:
module merges, generated units, pack rendering, runtime contracts,
and the Rust workspace's WebTransport / HTTP surface. Avoid asserting
facts already obvious from the literal config under test.

Reusable package derivations expose focused tests through
`passthru.tests.<name>`. Cross-package eval invariants live in
checks. Keep `checkPhase` or `installCheckPhase` for cheap checks
that should always run with the build.

When a change tightens source filtering, dependency identity,
generated derivations, or cache behavior, add a test that changes
one small input and proves the unrelated output remains unchanged.

## Searching

Use exact text search for exact questions and semantic search for
fuzzy questions. Prefer machine-readable output when available, then
inspect the narrow source files that own the behavior.

Avoid broad agent delegation for simple search. The codebase is
usually small enough that direct search plus a focused read gives
better signal.

Search before claiming external facts, API behavior, flags,
versions, or current ownership. Live state beats docs when the task
is about a running system; if observers disagree, debug the observer
path too.

Debug from first principles: actor, operation, boundary, invariant,
observer. Prove the broken boundary with the smallest live check,
then fix the owner.

## Layout

```
flake.nix                  # manifest: inputs + delegated outputs
elixir/                    # Symphony runtime (.sym/IR orchestrator)
workflows/                 # pack-agnostic example pack + operator packs
packages/room-server/      # Rust HTTP + WebTransport backend for the room app
packages/room/             # Svelte + Tauri desktop client (not Nix-built)
modules/services/          # NixOS modules for room and symphony
docs/                      # repo-owned reference
```

Folders should preserve conceptual paths. When siblings share a real
domain, nest them under that domain instead of flattening the name
into repeated dashed prefixes.
