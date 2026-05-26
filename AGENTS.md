# ix/images

Pre-built OCI images for ix VMs, plus composable NixOS modules. All images target
`x86_64-linux`.

This repo favors small, typed, reproducible surfaces. A good change leaves the
next reader with one obvious place to look, one command to run, and a failure
mode that names the real problem.

## Scope of AGENTS.md

`AGENTS.md` is for durable working principles. Add guidance here only when it
applies to a class of future changes across the repo, or when it captures an
architecture invariant that would be expensive to rediscover.

The test for a new rule is generality. It should survive the specific feature
that prompted it, apply to the next helper or module with the same shape, and
read more like a design philosophy than a task note. Specific examples are fine
when they sharpen the rule. The example should never be the rule.

Put local facts in the narrowest home that owns them: README files, option
descriptions, generated reference, issue bodies, module docs, or an inline
comment next to the load-bearing line. When a narrow note keeps growing across
features, promote the broad invariant here and leave the local details where
operators will look first.

Before adding durable guidance, search the tree and existing docs first. Facts
that are easy to rediscover with source search, generated reference, PR history,
or a narrow README should stay out of this file.

Each addition should be one or two direct sentences. Name the invariant, owner,
or decision rule, and include a path, command, URL, or external reference only
when it is the durable handle for that rule.

## Workflow

Pull `main` before starting. Always make changes on a short-lived branch in a
separate worktree by default, including small docs edits. Keep the shared `main`
checkout as the clean landing zone for pulls, branch bases, and final syncs.

Create the branch and worktree from the updated `main` checkout. Use the
`codex/` branch prefix unless the user asks for a different name:

```sh
git worktree add ../<short-name>-<branch> -b codex/<branch> main
```

If the shared checkout already has unrelated edits, name the paths and the one
line summary of what they appear to be doing before creating the new worktree.
Avoid stashing operator work out of the way.

After local checks pass, push the branch and open a PR targeting `main`. Enable
auto-merge as soon as required checks and review state allow it. Watch required
checks with `gh pr checks --watch --fail-fast`; if a check fails, inspect the
run logs, fix the branch, push again, and restart the watcher. Keep that loop
going until GitHub reports the PR merged or a human explicitly asks you to stop.

`gh pr checks` may show stale failed runs next to newer passing reruns for the
same check name. When the output is mixed, inspect
`gh pr view --json mergeStateStatus,statusCheckRollup,latestReviews` and trust
the latest run for the current head SHA rather than the oldest failure in the
list.

Treat PR comments and reviews as part of the work. Read them with
`gh pr view --comments` and the review fields from `gh pr view --json reviews`.
Address Codex comments in code when they identify a real issue, reply when a
comment is intentionally declined, and resolve review threads before relying on
auto-merge. Codex is the default code review signal for agent-authored PRs; do
not add or preserve a separate GitHub code-quality lane unless the user asks for
it.

Codex inline feedback lives in GitHub review threads, which `gh pr view
--comments` does not show. Inspect unresolved threads directly before deciding a
PR is clear:

```sh
gh api graphql \
  -f owner=<owner> -f repo=<repo> -F number=<pr> \
  -f query='query($owner:String!,$repo:String!,$number:Int!){ repository(owner:$owner,name:$repo){ pullRequest(number:$number){ reviewThreads(first:100){ nodes{ id isResolved path line comments(first:50){ nodes{ author{login} body url } } } } } } }'
```

Unresolved Codex review threads are immediate blockers. Do not wait on more
checks when Codex has left an open thread: fix the code or resolve the thread
with the GitHub review-thread API, then request a fresh Codex review if the head
changed. If the head did not change and GitHub does not rerun the failed gate,
rerun it with `gh run rerun <run-id> --failed`.

When manually triggering Codex, include the full current head SHA in the request,
for example `@codex review head <sha>`. This gives no-findings responses a
specific revision to answer. Avoid sending a later generic `@codex review` for
the same head because it weakens that audit trail.

Remove the worktree and delete the local branch after the PR has merged.

Commit one logical change at a time. Use the pathspec form so unrelated staged
or unstaged files cannot ride along:

```sh
git commit -m "scope: imperative subject" -- <paths>
```

Subjects are imperative, lowercased, and have no trailing period. The optional
scope names the layer being touched, such as `platform:`, `minecraft:`, or
`AGENTS:`. Use a body only for the reason the diff cannot show. If a commit
fixes a tracked GitHub issue, include `Fixes #123`, `Closes #123`, or
`Resolves #123` in the body. Use `Refs #123` for related or partial work.

`main` is the long-lived human branch. PRs target `main`. Deployment refs are
tags on commits that are already reachable from `main`.

Contributor setup and local checks live in [`CONTRIBUTING.md`](CONTRIBUTING.md).
Run the repo lint before committing:

```sh
nix run .#lint
```

Use the GitHub CLI credential helper for HTTPS pushes when the default helper
would reuse a read-only bot credential:

```sh
gh auth setup-git
git push -u <canonical-remote> <branch>
```

Choose the remote name that points at `indexable-inc/index`, such as `origin` in
the shared checkout or `upstream` in a fork-based clone. Keep the branch tracking
the same remote that received the push.

## Site updates

Operator-facing behavior changes should usually get one compact entry in
[`site/src/lib/updates.ts`](site/src/lib/updates.ts). Keep the first sentence
useful when read aloud and put exact links near the detail.

Keep checked-in site builds pure. The site should read text and static assets
from the repo without API keys, paid services, or network side effects. Generated
media, search indexes, catalogs, and similar artifacts belong behind explicit
commands or CI steps that write static outputs before the site build consumes
them.

Prefer a plain text feed before adding richer publication channels. Rich media
feeds need real media files with stable URLs before they are advertised.

## Writing style

These rules apply to prose in docs, READMEs, comments, issues, and PR
descriptions.

Start with the reader's task. A README opens with a short plain-language summary
directly under the title, then moves into task-specific headings. Keep paragraphs
short. Remove completeness theater.

Write in concrete nouns. Link the first mention of repo-owned tools, packages,
commands, directories, and important upstream projects in each section. Match
upstream capitalization: `nixpkgs`, `systemd`, `ix`, `pnpm`.

Use measured details where they matter. A number, command, file path, upstream
issue, or failure message earns more trust than a smooth adjective. Prefer "the
first build takes about 40 minutes" over "slow at first".

Name limits and failure modes. A short "bad fit if" or "known limitations"
paragraph often helps more than another claim of strength. Say what breaks, how
to notice it, and which workaround hurts.

Avoid slogan shapes that contrast a good phrase with a bad one, such as
`X, not Y` or `X, don't Y`. State the desired thing directly. Avoid em dashes;
split the sentence or use a colon.

Avoid balanced three-part cadence when it feels manufactured. Vary the rhythm:
two beats, four beats, a precise odd detail, or a short sentence with teeth.

## Inline comments

Comments explain why a line exists, which failure it prevents, or which external
constraint pins the choice. They should add information the syntax cannot
recover. Delete comments that narrate the code.

Leave a comment when something looks redundant but a build, eval, or test proves
it is load-bearing. Put the observed symptom next to the line that survives the
obvious cleanup.

Non-obvious technical decisions need a public reference when one exists: RFC,
JEP, upstream issue, vendor doc, benchmark, errata, or design note. Put the URL
in the comment near the choice. If no public reference exists, say where the
decision came from.

Public helpers exposed through the flake `lib` output or `specialArgs.ix` use
per-binding `/** ... */` doc-comments. Document the argument shape, return
shape, and observable behavior. Keep implementation-only comments for the "why"
notes above.

## Rust style

Repo-owned crates, fixtures, examples, and generated manifests use Rust edition
2024. Fix compatibility issues directly and document unavoidable upstream
blockers next to the exception.

Prefer names that preserve the concept's path. Local aliases may shorten noisy
source paths only when the shape remains visible at the call site. Keep singular
names for single values and plural names for bags of constructors, helpers, or
registry entries.

Use local type annotations when they make the data shape clearer. Keep turbofish
for expression-local cases where an intermediate binding would add noise.

Use normal module layout. Move files so `mod` declarations follow the filesystem
instead of using `#[path = ...]`.

Avoid anonymous tuple-shaped domain data once a value crosses a function
boundary. Prefer named structs or full paths for values that carry real meaning.

Use blank lines as paragraph breaks inside functions: set up, act, then validate
or return. Keep tightly coupled statements together.

When parsing, normalizing, serializing, traversing graphs, handling archives, or
speaking protocols, start from a maintained crate. Hand-written logic is for the
thin glue around that crate unless the dependency boundary is measurably worse.

## Python style

Default repo-owned Python apps to uv: `pyproject.toml`, committed `uv.lock`,
normal `src/<package>/` files, and Nix packaging through
[`ix.buildUvApplication`](lib/build-uv-application.nix).

Use [`ix.writePythonApplication`](lib/default.nix) for tiny single-file commands
without PyPI dependencies or multiple source files. Once a script needs
dependencies, console entry points, or a package layout, give it the uv project
shape.

The Python helpers run basedpyright in `standard` mode by default. Change the
type-checking mode only when the package has a deliberate reason.

## Sane defaults

Helpers, modules, packages, templates, examples, and generated commands should
be useful in the common production-shaped path without extra ceremony. Defaults
should be checked, typed, reproducible, conservative about secrets and
networking, and easy to override with a named reason.

Prefer the future-correct interface over compatibility layers. This repo can
change its own callers when an old spelling makes the safe path harder to
express. Remove migration branches and stale aliases in the same change that
introduces the clearer interface unless the user explicitly asks for a migration
window.

When the ecosystem already provides a robust tool for a large surface, push back
for at least one turn before rebuilding it here. Name the existing tool, the
maintenance cost, and the concrete gap that would justify ownership. If the gap
is real, track the work so the new surface keeps earning its weight.

For a small choice, lead with the direct answer and the shortest working path.
Save comparison tables for long-lived boundaries or vendor commitments.

Before finishing a change, reread the diff with suspicion. Ask whether the owner
is clear, whether a helper or type would remove real duplication, whether a
boundary is string-shaped when it should be typed, and whether a smaller API
would make the next change easier.

Fix root causes at the owner. When the same adapter, default, conversion, or
fallback appears in multiple places, move the capability transition or invariant
to the boundary that owns it.

Turn assumptions into checked behavior through types, schemas, module options,
derivation checks, or focused tests. If the user asked for a fix, land code and
the nearest durable test or validation hook; diagnosis alone is unfinished work.

Delete vestigial code in the same change that makes it obsolete. Dead fields,
options, structs, functions, configs, generated files, and compatibility shims
make the safe path harder to see.

When adding a non-obvious workaround, policy exception, or operational guard,
put the reason near the choice and cite a durable source when one exists.

## User-facing commands

Keep protocol emitters separate from product workflow code. Workflows should
produce facts; terminal, API, and document surfaces should render those facts
for their audience.

Human-readable output is the default for interactive commands. Agents, scripts,
and tests should prefer machine-readable output when the command supports it.

Long-running commands should expose the phases users naturally ask about.
Terminal progress should keep moving while work is in flight, with recent rate
and cumulative totals reported as separate facts when both matter.

Default errors should lead with the operator-facing failure and actionable
context. Source locations, backtraces, trace paths, and internal module paths
belong behind debug output or structured output unless they are the user's next
step.

## Nix philosophy

`flake.nix` is the manifest. It should expose inputs and delegate outputs to
`lib/`, discovery, and package-specific files. Keep scenario wiring, artifact
manifests, app wrappers, and helper logic near the owner that changes with them.

Use standard flake outputs: `packages`, `checks`, `formatter`, `devShells`,
`templates`, `overlays`, `nixosModules`, and `lib`. A workflow command should be
a package with `meta.mainProgram` so `nix run .#<name>` and
`nix build .#<name>` point at the same derivation.

Composition belongs in this repo; low-level ix VM primitives belong in `ix`.
Build workflows by consuming stable primitives, rendered plans, and plain data
surfaces. Add CLI primitives only when the lower layer truly owns the behavior.

Expose aggregate knowledge as data before wrapping it in a command. A
`lib.<name>` value that `nix eval --json` can inspect is easier to reuse than a
one-off app. Add a wrapper when formatting, joins, follow-up actions, or
human-facing output justify it.

Prefer one source of truth. Discovery beats hand-maintained registries. Generated
catalogs should come from small manifests. Hashes live with URLs. Versions live
near the image, package, or ecosystem that owns them.

Keep eval pure. Inputs flow through `flake.nix` or typed parameters. Avoid host
environment reads, channel refs, ad hoc flake paths, and eval-time network
fetches.

Import From Derivation is acceptable when another tool must reveal the real
build graph. Keep the boundary explicit, expose the generated artifact, and
batch discovery into one larger derivation when many tiny IFDs would serialize
the evaluator.

Generate commands through checked helpers. A wrapper reached through
`nix run .#...` should call realized executables with `lib.getExe`, an app
program, or an explicit store path reference. Avoid nesting another flake
frontend inside the generated command.

Nix builders for language workspaces should pass the smallest source closure the
compiler can consume. The caller names both the filtered `src` and the real
`workspaceRoot`; do not infer one from the other.

Nix source filtering and flakes only see tracked or staged source files. Stage
new source files before running Nix validation so failures describe the
expression under test instead of a missing path.

Start validation narrow, then broaden as confidence grows. Package invariants
belong in the owning derivation through `checkPhase`, `installCheckPhase`, or
`passthru.tests`; keep flake `checks` as aggregation and policy gates.

## Module conventions

Modules declare options and config. Keep each module inert until its enable flag
or equivalent activation condition is set. Prefer independent modules registered
through the central registry over modules importing each other.

Public options should describe the user's domain. Hide storage mechanics behind
typed options, generated files, and small adapters. Use broad escape hatches only
at true foreign-format boundaries and name that boundary in the description.

Structured config belongs in structured values. Prefer `pkgs.formats.*`,
freeform submodules, and typed option trees over string fragments that cannot
merge, inspect, or receive `mkDefault` and `mkForce` cleanly.

Cross-cutting helpers come through `specialArgs.ix` or the public flake `lib`
surface. Avoid relative-up imports that climb across repo layers. Child and
sibling paths inside one package or module directory are fine.

Service families share a runtime module plus variant modules that fill typed
slots. Enabling a variant should enable the runtime by default. Mutually
exclusive variants should fail loudly through module merging or explicit
validation.

Every module that binds a TCP or UDP socket should declare a port claim next to
the bind setting or firewall declaration. A duplicate claim in the same
namespace is a useful eval-time failure; intentional co-location needs a real
namespace boundary or an explicit alternate port.

Modules that manage artifacts should consume catalogs, lockfiles, or caller
supplied sources. Presets and examples should read like intent, with local or
private artifacts shown only when that is the point of the example.

## Image conventions

An image is an independent NixOS system closure packaged as an OCI archive:
systemd as PID 1, no kernel, no bootloader. Images are not stacked at runtime;
layering is a build and registry storage concern.

Design for ix VM assumptions. Disks can grow very large, snapshots are normal,
and nodes can have substantial CPU and memory. Use limits to contain runaway
services, preserve rollback, and keep operations legible. Avoid shrinking useful
operator tooling merely to save a small closure delta.

Do not add at-rest encryption inside images as a default. ix storage deduplicates
guest blocks, and guest-side encryption turns identical data into random bytes.
If a workload has a real compliance requirement against the host, name that
requirement and design a separate channel for it.

Treat a root process inside the VM as fully capable inside the guest. Anything
that must hold against that process belongs outside the VM: host credentials,
registry-write tokens, snapshot authority, source-switch authority, and hard
network containment.

Use image networking for cooperative guest intent. Per-port firewall rules,
service frontends, and local mTLS belong in the image or a gateway VM. Policy
that must resist a compromised guest belongs in a router, gateway, group
boundary, or host-side primitive the guest cannot edit.

All images target `x86_64-linux`. Host-visible flake package namespaces may
exist for developer systems, but image derivations still build Linux systems.
Use generic nixpkgs packages when possible so upstream caches substitute.
Service-specific hardware tuning belongs in the module where the operator can
see the tradeoff.

Use topology for same-protocol public port conflicts. Put services that need the
same natural port in separate nodes, use an explicit alternate port, put a real
frontend in front of them, or create a true namespace boundary. Runtime "pick any
free port" behavior makes docs, firewalls, health checks, and fleet plans lie.

Do not assume registry images are public. System namespaces may publish public
bootstrap images; user namespaces default to private and should behave like
not-found for other users. Debug access before treating a pull failure as an
outage.

Platform-wide defaults have two homes. System posture lives in
[`lib/ix-platform.nix`](lib/ix-platform.nix). Operator ergonomics and shared CLI
tools live in [`modules/profiles/base/`](modules/profiles/base/). Use
`lib.mkDefault` when an unusual image might need a one-line override.

Add a new image by adding a NixOS module at
`images/<category>/<name>/default.nix`. Discovery exposes the package on the
next eval. A versioned image keeps variants in a sibling `versions.nix`, with one
default variant and one small data record per version.

Images and presets should use one coherent `services.<name>` block per service.
Nest sub-options under that block so the configuration reads like the service
shape rather than a scatter of dotted assignments.

Presets should own intent. Artifact URLs, hashes, generated metadata, and broad
catalog data belong to the nearest update mechanism that can refresh them
mechanically. A preset may show a local or private artifact when the example is
about that override.

## Layout

```
flake.nix                                  # manifest: inputs + delegated outputs
.envrc, .githooks/pre-commit               # direnv wires the tracked hook
lib/                                       # public helpers, builders, discovery
modules/                                   # registered NixOS modules and profiles
images/                                    # image modules plus optional versions
nix-rules/                                 # ast-grep lint rules
```

Folders should preserve conceptual paths. When siblings share a real domain,
nest them under that domain instead of flattening the name into repeated dashed
prefixes. Published package names, image tags, and upstream identifiers can keep
their external spelling.

Move a legacy flat path while doing nearby work when the rename is small and
call sites are inside the repo. Leave a follow-up when the rename is larger than
the work that exposed it.

## Dependency intake

Every external input needs an owner that can update it predictably. Prefer
ecosystem lockfiles, flake inputs for real flake-level tools, repo manifests
consumed by updaters, or narrow `pkgs.*` fetchers when no better owner exists.

The human workflow is: edit the source requirement or manifest, run the owning
updater, inspect the generated diff, and commit the source and generated
hash-bearing output together.

Use the most specific `pkgs.*` fetcher for the source: `fetchurl` for opaque
single files, forge fetchers for forge snapshots, `fetchgit` for raw git refs,
`fetchzip` for archives that must unpack, and ecosystem fetchers when one
exists. Avoid `builtins.fetch*` in tracked Nix files because those fetch during
eval and do not substitute like fixed-output derivations.

Tracked Nix files should never contain fake hash helpers or placeholder hashes.
Materialize real SRI hashes with the owning updater, lock command,
`nix flake update`, or a checked prefetch command before committing.

Use `__impure` only for explicit dependency-discovery or prefetch derivations
that are turned into a checked hash-bearing artifact before product builds
consume them. Keep the impure boundary named next to the updater or generated
lock output that makes later builds pure.

Generated catalogs are build inputs, not hand-edited source. If a generated file
is wrong, change the manifest or generator that owns it.

Keep binary and generated artifacts near the owner that can explain and refresh
them. Use small manifests for curated sets, generated catalogs for URLs and
hashes, and metadata catalogs for search or browsing surfaces.

Repository examples should consume those shared surfaces. Repeating URLs and
hashes in examples creates second owners with no update story.

## Nix practices to tighten

Improve these patterns when touching nearby code. If cleanup is wider than the
task, file a narrow issue.

- Prefer precise option types over broad attrs. Keep broad attrs at true foreign
  format boundaries.
- Filter local sources to the smallest useful tracked file set.
- Use `lib.getExe` or `lib.getExe'` instead of spelling `${pkg}/bin/foo`
  repeatedly.
- Keep validation in shared builders and reuse those builders everywhere.
- Fix the improper layer when stricter validation exposes a helper problem.
- Use checked Nushell helpers for non-trivial generated commands.
- Keep new scripts in a language that matches the data shape they handle.
- Avoid generated `nix run` wrappers that call `nix run`, `nix build`, or
  `nix flake check` internally. Model dependencies as derivation inputs or keep
  orchestration outside Nix.
- Default to no `devShells.default`; add per-package shells or build inputs where
  the need belongs.
- Keep the tracked pre-commit hook as a small entry point to the lint app.

## Nix style (ast-grep enforced)

Run `nix run .#lint` before committing. It runs nixfmt, Statix, Deadnix, and the
repo's ast-grep rules. The lint app is the mechanical source of truth. The
common hard rules are:

- No `with pkgs;` or `with lib;`. Use `inherit (pkgs) ...` or `lib.foo`
  directly.
- No `rec { }`. Use `let ... in` or `final` / `prev`.
- No `mkForce`. Fix the module boundary or compose priorities deliberately.
- No `lib.recursiveUpdate`. Build the attrset in one place or use `lib.mkMerge`.
- No repeated parent keys in the same attrset. Group related assignments under
  one parent.
- Prefer `inherit (source) name;` for direct same-name field copies.
- No `builtins.currentSystem`, `builtins.getEnv`, `<nixpkgs>`, or `path:` flake
  refs.
- No `(import ./foo.nix)` inside `imports = [ ... ]`; NixOS auto-imports paths.
- No `..` paths inside `modules/`; shared helpers come through `specialArgs.ix`.
- No `writeShellApplication` or `writeShellScriptBin` for user-facing commands.
- No bare `assert cond;`. Use an assertion that names the failure.
- No unused bindings. Use `_` for intentionally unused lambda arguments.
- Set `strictDeps = true` on every `mkDerivation`.
- Keep raw fetched data artifact URLs out of `flake.nix`.
- Use `pkgs.*` fetchers instead of `builtins.fetch*`.
- Commit real hashes, never fake hash helpers or placeholders.
- Use `nixosModules.<name>` for module exports. Avoid a flat top-level
  `modules` output.
- Keep image targets at `x86_64-linux`.
- Use structured config options for new modules instead of stringly config
  fragments.

## Issues

Keep issue bodies short: problem, context, desired outcome. Bug reports need a
concrete reproduction command or step list. Avoid prescribing implementation
unless that is the actual request.

When creating or editing GitHub issue bodies or comments, pass multiline text
through a real multiline input path such as `--body-file -`, a temporary file, or
an editor. Escaped `\n` sequences in quoted `--body` strings render literally on
GitHub.

Prefer GitHub's suggestion block syntax for proposed inline changes in PR review
comments on changed lines. Use fenced `suggestion` blocks only when GitHub can
apply the snippet directly.

When work exposes a real bug, broken assumption, or unidiomatic pattern that
will outlive the current task, file a GitHub issue right then. One concrete
observation per issue.

Apply labels at filing time. Use labels to make the next action sortable:
`bug`, `enhancement`, `documentation`, `rfc`, `help wanted`, `good first issue`,
and `ai-capable` when an agent can plausibly finish the issue from the body
alone.

## Tests

Tests should protect behavior that can regress across boundaries: module merges,
generated units, fleet rendering, artifact wiring, security posture, and runtime
contracts. Avoid asserting facts already obvious from the literal config under
test.

Image and reusable package derivations expose focused tests through
`passthru.tests.<name>`. Cross-image eval invariants live in checks. Keep
`checkPhase` or `installCheckPhase` for cheap checks that should always run with
the build.

When a change tightens source filtering, dependency identity, generated
derivations, or cache behavior, add a test that changes one small input and
proves the unrelated output remains unchanged.

## Searching

Use exact text search for exact questions and semantic search for fuzzy
questions. Prefer machine-readable output when available, then inspect the narrow
source files that own the behavior.

Avoid broad agent delegation for simple search. The codebase is usually small
enough that direct search plus a focused read gives better signal.

Search before claiming external facts, API behavior, flags, versions, or current
ownership. Live state beats docs when the task is about a running system; if
observers disagree, debug the observer path too.

Debug from first principles: actor, operation, boundary, invariant, observer.
Prove the broken boundary with the smallest live check, then fix the owner.

## Debugging VMs

Use the real ix CLI to inspect running VMs before inferring from source. Prefer
machine-readable host commands when available, such as `ix ls --output json`.

Run guest commands with `ix shell <vm> -- <cmd> ...`. If command lookup differs
from an interactive shell, use absolute paths from the guest.

For service failures, check the rendered unit and the live journal inside the
VM. Confirm the unit exists, PID 1 is systemd, and the process is failing after
launch before changing image or module code.

When a debugging tool is missing on the host or in the dev shell, run it through
nixpkgs with `nix run nixpkgs#<tool> -- ...` instead of hand-installing it.

## Linting

```sh
nix run .#lint
```

The tracked pre-commit hook runs the same lint app. CI runs the same check
through the flake. Keep one lint entry point so local and CI failures mean the
same thing.
