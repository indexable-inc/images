# ix/images

## Workflow

Commit and push after making changes by default.

Use `git commit -m "..." -- <paths>` to commit a specific set of files atomically, instead of `git add` followed by `git commit`. Everything after `--` is treated as a pathspec, so flags like `-m` must come before it. The pathspec form takes the working-tree contents of just those paths in one step, so an unrelated staged or unstaged change in another file cannot accidentally ride along. Use `git add` only when you genuinely need to split one file's hunks across separate commits (`git add -p`).

One logical change per commit: a refactor, a behavior change, a doc note, and an unrelated fix each get their own commit. Bundling a mechanical fixup (formatter run, lint-required rename) with the change that triggered it is fine; bundling two independent policy changes is not. Subjects are imperative, lowercased, no trailing period, with an optional `scope: ` prefix that names the touched layer (`platform:`, `AGENTS:`, `minecraft:`). Bodies are for the *why* the diff cannot show: motivation, tradeoffs, the constraint that survives an obvious-looking refactor. Skip the body when the subject already says everything; never write a body that just paraphrases the diff. Avoid `fix stuff`, `WIP`, `address review feedback` (name the feedback), and mixed-concern subjects like `format + add feature`.

Contributor setup and local checks are in @CONTRIBUTING.md.

Work directly in the shared checkout on `development` unless the user asks for a branch or separate worktree. Pull before starting, then commit and push to `development` after checks pass.

When the shared checkout already has unrelated in-progress edits, name them (paths and a one-line summary) and move the new work into a worktree (`git worktree add ../<short-name>-<branch> -b <branch> development`). Do not `git stash` the WIP out of the way: stashing is silently destructive from the working operator's perspective and ties the new work's success to a clean unstash later. Land worktree commits with `git push origin <branch>:development` (the pre-commit lint runs over the worktree, so it stays green even when the main checkout is dirty), then `git worktree remove ../<short-name>-<branch>` and `git branch -d <branch>`.

### Branch model

`development` is the GitHub default branch. PRs target `development`; routine work lands there after local checks and review.

`main` is the promoted consumer branch. Pin examples, downstream flakes, and image consumers to `main` when they need a commit that has passed the cache gate. Human changes should flow through `development`.

A scheduled workflow fast-forwards `main` from `development`. It selects the newest successful `Check` workflow run on `development` that completed at least 6 hours earlier, re-runs `nix flake check`, then pushes `main` to that commit. The 6-hour pause is a supply-chain speed bump: a lockfile can pin a compromised artifact before GitHub, Modrinth, crates.io, npm, or an upstream maintainer has reacted. The pause gives takedowns, advisories, and cache churn time to show up before consumers follow the branch. Keep using the 24-hour dependency-intake default for routine third-party bumps; the branch delay is the final gate for already-merged development commits.

When a commit actually fixes a tracked GitHub issue, include an auto-closing keyword in the commit body, for example `Fixes #123`, `Closes #123`, or `Resolves #123`. Use `Refs #123` only for related work, policy docs, investigation, or partial cleanup that should not close the issue.

## Writing style

These rules apply to prose in docs, READMEs, comments, issues, and PR descriptions.

README files are written for human scanning. Machine-readable detail belongs in
typed options, generated reference, or linked source files. Assume a reader with
severe ADHD is trying to decide what this thing is and what to do next. Start
with a short plain-language summary directly under the title, then move into
task-specific headings. Do not label the opening summary; the first paragraph
should do that work. Keep paragraphs short and remove completeness theater. The
README should leave the reader oriented, not exhausted.

Do not use the "X, not Y" or "X, don't Y" rhetorical pattern. It is filler that reads as marketing. State what the thing is in a positive form, drop the contrast. Replace "ix VMs without the plumbing, not glue" with "ix VMs with services that compose." Replace "Compose, don't glue" with "Compose services."

Do not use em dashes. Write so the sentence does not need one: split into two sentences, use a colon, use parentheses, or restructure. If the urge is to insert "—", the sentence is doing two jobs and should be cut into two.

Avoid the rule of three. Tricolons like "one lockfile, one catalog, one source of truth" or "fast, cheap, reliable" read as 2024-era LLM cadence and have become cliché. By 2026 readers have shifted toward out-of-distribution phrasing with a little friction in it: uneven list lengths, unexpected concrete nouns, sentences that resolve at two beats or at four. Prefer two-part or four-part structures, mix clause lengths, and reach for a specific surprising detail instead of a third parallel slot. If a sentence falls naturally into three balanced clauses, suspect the cadence and rewrite.

Lean into small oddities. A line of prose should have one or two touches that a default LLM would not have produced: a trademark symbol on a brand name where most writers would drop it (`LEGO™`, not "Lego"), the actual proper-noun product instead of a generic synonym (`Postgres` over "a relational database"), an idiosyncratic capitalization the upstream project itself uses, a precise verb where a vague one would scan fine. The goal is texture that signals a human made specific choices, not polish that signals a model averaged its training data. Do not manufacture quirk for its own sake; let the oddity come from being more accurate, more specific, or more brand-faithful than the smooth version would be.

Other oddities worth borrowing, all downstream of one principle: commit to a specific thing rather than a smooth abstraction.

Use concrete numbers in place of qualifiers. "~67 OCI layers" beats "many layers"; "the first build takes about 40 minutes" beats "slow at first". A number commits you to having measured, and an approximate `~67` is more honest than "lots". Vague intensifiers like "robust", "scalable", "performant", or "battle-tested" commit to nothing and read as a model averaging its training data.

Anti-marketing earns trust. A short "bad fit if" or "known limitations" paragraph signals the writer has used the thing in anger and survived its failures, so the reader trusts the rest of the page. List the cases where the project is the wrong choice; name the workarounds that hurt.

Use parenthetical asides with teeth. A real-opinion clause set off in parens lets you hedge without softening. (The LEGO™ metaphor falls apart the moment two services want port 25565.) Default LLM writing either commits fully or smears the hedge across the main clause with "may sometimes"; parens admit the edge case while keeping the main assertion sharp.

Match upstream's own capitalization. `nixpkgs`, `systemd`, `ix`, `znver5`, `pnpm`. Title-casing project names as if they were normal English nouns ("Nixpkgs", "Systemd") signals the writer learned about them from a tutorial, not from the project's own README.

Name the failure mode you are claiming to avoid. "Will not crash your VM on a bad switch" is more useful than "stable"; "recovers in one rollback" beats "reliable". Failures are concrete and falsifiable. Adjectives like "stable" and "reliable" are not.

Link concrete nouns. When prose names a repo-owned tool, package, command, file, directory, or upstream project, make it a link wherever a curious reader might want to click through. The first mention of `llm-clippy` in a README should link to `packages/llm-clippy/`; `nixpkgs` links to the project; a directory pointer like `images/` links to `images/`. Anchor text is the literal name in code voice (backticks live inside the link, as `[`llm-clippy`](packages/llm-clippy/)`). Link only the first mention in a given section; do not relink the same noun every paragraph. Skip the link only when the target genuinely does not exist or when the reader is already standing on it (no self-links in the same file).

## Inline comments

Inline comments are scarce by default. Reserve them for cases where the code does not explain itself: a workaround whose reason lives outside the file, a value that looks wrong but is load-bearing, a choice that survives an obvious-looking refactor for a real reason, a number whose origin would otherwise be lost. If you'd answer "why is this here?" with anything other than "read the code", leave a one-line `#` (or `//`, `--`) comment.

Do not narrate what the code does. `# install the package` above `pkgs.install ...` is noise the next reader has to skip. The comment should add information the syntax can't recover: an upstream bug link, the failure mode that motivated the workaround, a benchmark number, a date.

Concretely: if you find yourself about to delete something because it looks redundant, and then have to put it back because a build, eval, or test breaks, that is the canonical case for an inline comment. Add it the moment you discover the constraint, name the symptom you observed ("ERROR: Missing Cargo.lock from src" beats "needed for build"), and keep it next to the line whose existence it justifies.

Non-obvious technical decisions take a public reference inline next to the line. A GC choice, an allocator pick, a default JVM flag, a kernel tunable, a memory budget, a chosen algorithm constant: include the JEP, RFC, vendor doc, benchmark write-up, or upstream design doc that anchors the claim, with the full URL in the comment. The link is what lets a future reader verify the claim still holds and notice when it has been superseded by a newer JEP, errata revision, or upstream change. Pin the citation to the exact source: name the JEP number, the kernel commit hash, the errata id, the benchmark page, not a forum post that mentions it secondhand. Internal references (PR numbers, Linear tickets, incident IDs) describe what we did but do not on their own justify the choice. When no public source exists for the exact claim, say that explicitly ("no public benchmark for X; this default came from internal load tests on workload Y") so silence does not read as the absence of a reason.

## Rust style

Prefer local type annotations over turbofish when they make the data shape clearer. For example, use `let args: Vec<_> = env::args().collect();` instead of `let args = env::args().collect::<Vec<_>>();`. Keep turbofish for cases where an expression-local type is genuinely clearer, such as method chains where naming an intermediate value would add noise.

Do not use Rust `#[path = ...]` to paper over module layout. Move files so the filesystem hierarchy matches normal `mod` declarations.

Avoid anonymous tuple-shaped domain data. Prefer named structs or full paths when a value crosses a function boundary or represents a real concept. Small local tuples are fine when the scope is obvious.

Use descriptive names as scope widens. Short loop names such as `i` or `_` are fine in a few-line scope, but use names like `path`, `bytes`, `config`, `request`, and `response` once the value survives long enough to need meaning.

Avoid parallel alias sprawl: do not create several sibling locals that repeat or flatten the same subject with different suffixes (`paperConfig`, `paperCfg`, `paperService`, `paperServiceConfig`, `gitCloneService`, `rconOpenFirewallConfig`). That is a data-clump smell. Group the subject once and nest both the root value and derived handles under it so call sites read by path instead of by duplicated names.

Be strict about preserving the conceptual path in local test fixtures and Nix `let` blocks. A binding may shorten a noisy source path only if the alias keeps the same shape: `minecraft.paper.config`, `minecraft.paper.service.unit`, `minecraft.paper.service.config`, `minecraft.rcon.openFirewall.config`, `kernelDev.git.clone.service`. Do not collapse path segments into camelCase or sibling suffixes: avoid `paperConfig`, `paperService`, `paperServiceConfig`, `rconOpenFirewallConfig`, `kernelDevGitCloneService`, and `gitCloneService`. Promote a binding to the surrounding scope only when it is genuinely independent of the owner object.

Use names like `cfg` only for the conventional Nix module option subtree (`config.services.<name>`), not as a catch-all alias for any nested value. For systemd units, keep `unit` and `config` separate under `service` (`service.unit` and `service.config`), rather than introducing `serviceConfig`.

Apply the same rule to generated files, managed paths, and fixtures. Prefer `minecraft.paper.managed.serverFiles` and `minecraft.paper.managed.dropins` over `paperManagedFiles` or `paperManagedDropins`. If the source path has meaningful segments, keep those segments visible in the alias path.

Name a binding by what it holds. A binding that points at a single typed value or a hierarchical config tree keeps the singular path (`minecraft.paper.config`, `node.service.unit`). A binding that points at a bag of constructors, helpers, or registry entries goes plural: `tags = ix.minecraft.nbt;`, `types = lib.types;`, `formats = pkgs.formats;`, `licenses = lib.licenses;`, `maintainers = lib.maintainers;`. The call site reads naturally as "from the set, pull the X": `tags.short 20`, `types.attrsOf types.str`, `formats.json { }`. The repo and nixpkgs already follow this; the rule is here so reviewers can point at a single line when the alias drifts. Singular forms of collection bindings (`tag`, `type`, `format`) read as field accesses on one thing rather than constructor picks from a set, so avoid them. A binding name pulled from outside the source path (`tags` for `ix.minecraft.nbt`) is acceptable only when it is the plural of what the namespace holds; if the path tail is already plural and apt, reuse it (`packages = ix.packages;`, not `pkgs = ix.packages;`).

Use blank lines as paragraph breaks inside function bodies. Each paragraph should be one logical step: set up, act, then validate or return. Keep tightly coupled statements together.

For snippets in docs, comments, presets, or task descriptions that readers may see without IDE inlay hints, include explicit types on important bindings and use real repo APIs rather than invented simplified ones. In source files, use inference where it reads cleanly.

For Rust tasks, start from a well-maintained crate when the problem is parsing, normalization, graph traversal, serialization, archive formats, protocol handling, or other domain logic with existing ecosystem coverage. Hand-written logic is fine for tiny glue around a crate API, but do not reimplement URL, TOML, semver, lockfile, or Cargo-style parsing unless the crate boundary is measurably worse for this repo.

## Sane defaults

Helpers, modules, packages, templates, examples, and generated commands should be in a good state by default. Defaults should be checked, typed, reproducible, reasonably locked down, and fit for the common production-shaped path. Callers should opt into weaker behavior with a named reason.

Examples should show intent and uncommon choices. Helpers and modules should carry boring safety such as typechecking, validation, durable state, hardening, and conservative networking. If a consumer has to spell a boring default to look correct, fix the helper or module.

When work exposes recurring pain and the fix belongs in `AGENTS.md`, capture the broad rule first. Write the invariant future tasks can reuse, then name the current case only when it adds clarity. A policy note should survive the next helper, language, or module with the same failure mode.

Prefer the future-correct interface over compatibility layers. This repo can break callers when an old helper surface makes the safe default hard to express. Remove old spellings, migration branches, and dual-mode behavior in the same change that introduces the new invariant unless the user explicitly asks for a migration window.

Nix builders for language workspaces should scope source inputs to the smallest source closure the compiler can consume. For Cargo unit builders, each `rustc` unit gets one package source: a local workspace member root or one vendored crate directory, plus only the symlink targets that package actually reaches and the directories its `.rs` sources reach through `include!` / `include_bytes!` / `include_str!` macros with literal string arguments (resolved relative to each source file, clamped to the workspace root). Computed include paths (`concat!`, `env!("OUT_DIR")`, identifiers) cannot be resolved statically and stay outside the closure; the caller has to surface them through the build script or a symlink the walker can follow. Callers pass the filtered build input as `src` and the real source boundary as `workspaceRoot`; do not infer one from the other. Key git dependency hashes by the exact `Cargo.lock` source string, including the locked rev. Package-name keys and unit-graph ids lose identity for multi-package git repos such as `snafu`. Treat a generated derivation that references the monorepo root or aggregate `cargo-vendor-dir` from every unit as a cache invalidation bug. Add eval checks that compare `.drvPath` before and after a one-crate source edit, plus a separate check for a one-dependency `Cargo.lock` change.

## Don't reinvent without measured benefit

When the user proposes building something the ecosystem already provides in robust form (a DAG runner, a process supervisor, a config language, a build system, a deployment tool, a CLI framework, a serializer), advise against it for at least one turn before doing any work. State the existing tool, the maintenance cost the user is taking on, and the specific gap that justifies the rebuild. Comply only after the user names that gap concretely: additional consumers the rebuild would consolidate, or behavior the upstream genuinely cannot reach. "We'd like to own it" and "it would be nicer" do not clear the bar. If the user pushes through anyway, file the work as a tracked issue so the rebuild has to earn its weight against later evidence, rather than landing it as a one-off spike.

The opposite mistake is also wrong: do not turn a small choice into an option-tree exercise. When the user asks "can I use X for Y", lead with the direct answer and the smallest concrete path. Reserve option-comparison tables for surfaces that will outlive the task, such as an interface boundary or a vendor commitment. For a one-screen task, "use X this way" beats listing X, Y, and Z side by side.

## Why `dag-runner` (and not `process-compose` or `devenv-tasks`)

The repo-owned [`dag-runner`](packages/dag-runner/) is the task runner that powers `nix run .#health-checks` and is the planned replacement for `ix-fleet`'s sequential per-node loops. It exists despite the rule above because neither upstream candidate fit cleanly:

- **`process-compose`** has only two output shapes: full-alt-screen TUI or plain text. The TUI takes over the terminal and clears its alt-screen on exit, so a fast failure leaves nothing in scrollback. The maintainer explicitly closed the door on inline-progress (`F1bonacc1/process-compose#362`). The model is "long-running supervisor", so one-shot semantics require a `_done` sentinel and `exit_on_end` to coax it into terminating.
- **`devenv-tasks`** standalone has the right UX shape (inline indicatif spinners, scrollback-friendly, exits on completion), but its JSON spec is an internal interface to devenv's module system and shifts with devenv. The binary is not in `nixpkgs` as a standalone, so consuming it pulls the whole devenv flake closure for one Rust binary.

What earned the build:

1. The current consumer is `health-checks`. The wait-in-the-wings consumer is [`ix-fleet`](packages/ix-fleet/src/ix_fleet/__init__.py)'s sequential `for node in selected_nodes(...)` loop in `cmd_up`/`cmd_switch`/`cmd_replace`. Re-expressing those imperative loops as DAG specs collapses repeated machinery and surfaces a uniform JSON event stream.
2. The JSON spec is small and stable: a flat map of `{ name → { command, depends_on } }` owned by this repo, not someone else's release cycle.
3. The runtime is bounded: roughly 400 lines of Rust on top of `tokio` + `indicatif` + `clap` + `serde_json`. Maintenance cost stays small.

If a third consumer arrives needing something dag-runner does not (per-node soft timeouts, conditional skips, cron-like scheduling), extend the runner. Pulling in `process-compose` or `devenv-tasks` alongside it for "the part dag-runner doesn't do" would defeat the consolidation. Grow one runner, not two.

## Python style

Default Python projects to uv. A repo-owned Python app has `pyproject.toml`, a committed `uv.lock`, normal `src/<package>/` files, and Nix packaging through [`ix.buildUvApplication`](lib/build-uv-application.nix). Keep dependency resolver diffs reviewable: change source and lockfiles together only when the source change needs the dependency move.

`ix.buildUvApplication` and `ix.writePythonApplication` run basedpyright in `standard` mode by default. Pass `typeCheckingMode` only when the package needs a deliberate stricter or narrower mode.

Use `ix.writePythonApplication` for tiny single-file commands with no dependency graph to lock. Once a script needs PyPI packages, console entry points, or multiple source files, give it the uv project shape.

## Debugging VMs

Use the real ix CLI to inspect running VMs before inferring from source. Prefer machine-readable host commands when available, for example `ix ls --output json`.

Run guest commands with `ix shell <vm> -- <cmd> ...`. If command lookup behaves differently from an interactive shell, use absolute paths from the guest, for example `ix shell minecraft -- /run/current-system/sw/bin/journalctl --no-pager -u minecraft.service -n 80` or `ix shell minecraft -- /bin/sh -lc 'ps -ef'`.

When a debugging tool is not installed on the host or in the dev shell, run it through nixpkgs instead of hand-installing it, for example `nix run nixpkgs#jq -- --version` or `nix run nixpkgs#curl -- --version`. Put arguments after `--`.

For service failures, check the rendered unit and the live journal inside the VM. Confirm whether the unit exists, whether PID 1 is systemd, and whether the process is failing after launch before changing image/module code.

## Overview

Pre-built OCI images for ix VMs, plus composable NixOS modules. All images target AMD EPYC Gen 5 (Turin, Zen 5). The base layer sets `nixpkgs.hostPlatform.gcc.arch = "znver5"` so every package in the closure is compiled with `-march=znver5 -mtune=znver5`. No binary cache hits: everything builds from source.

## How it works

Every image is an independent NixOS system closure: `boot.isContainer = true`, systemd as PID 1, no kernel, no bootloader. `lib.mkImage` runs `nixpkgs.lib.nixosSystem` over the platform config (`lib/ix-platform.nix`), OCI packaging (`lib/ix-oci-layer.nix`), the module registry (`modules/`), and any caller modules, then packages the toplevel into an OCI archive via the nixpkgs layer planner plus the repo's direct OCI archive builder (`lib/build-oci-image.sh`).

Images are not stacked at runtime. ix runs one image. Layering is purely a build-time concern: the closure is split into ~67 OCI layers so the registry stores each shared store path once and clients only pay for deltas. Single-layer would force every image to ship a private copy of the multi-hundred-MB base closure.

## VM assumptions

ix VMs implicitly have snapshots and effectively unbounded disk: the per-VM disk autoscales up to ~1 PiB on demand, so caps on journal size, coredump retention, GC roots, and similar bookkeeping exist to bound a misbehaving service rather than to save space. Fleet and stateful-service designs should lean on those primitives: take snapshots before destructive or data-format-changing operations, prefer in-place NixOS/system switches for stateful nodes, and do not design around fixed-root-disk exhaustion as a primary constraint.

These are proper VMs, not containers in the resource-constrained sense. A given node can have hundreds of GiB of RAM, many cores, and large attached disk, and the fleet autoscales node sizes to the workload. Design for that envelope: a few tens of MiB of closure for an extra dev tool, a long-running `determinate-nixd` helper, or an in-image cache is rounding error against a 256 GiB RAM Minecraft host. The container-shaped flags (`boot.isContainer = true`, shared linux-ix kernel) are about runtime model, not about being squeezed for resources. Image authors should pick conveniences and operator ergonomics that hold up at this scale rather than micro-optimizing closures.

Do not enable encryption in images. ix's storage layer deduplicates blocks across guest disks; encrypted blocks are random by design and defeat dedupe, so turning on LUKS, fscrypt, systemd-homed, BTRFS native encryption, or per-service at-rest encryption (Postgres TDE and similar) forces the user to pay for full per-VM storage instead of the deduplicated cost. The trust model already assumes the underlying ix host is safe; durable bytes live where the agent cannot reach them. Transport-level TLS on the wire is unaffected. If a workload truly needs at-rest encryption against the host (a real compliance ask, not a habit), name that requirement so the design can route it through a separate channel.

The same dedupe means closure size is largely free for common dev tooling. A 50 MiB editor or CLI replacement that lands in every image's base closure stores once globally and shows up as small per-VM delta, since cross-tenant cross-VM dedupe is collapsing identical store paths the moment a second image references them. Helix, neovim, micro, ripgrep, fd, bat, eza, htop, fzf, and the rest of the modern operator toolbox should ship by default; do not omit a tool because it adds tens of MiB to the closure. Reserve the closure-shrinking instinct for genuinely large or service-specific payloads (JDKs only the Minecraft loader uses, datasets, vendored frameworks), where the dedupe still helps but the per-image opt-in pattern keeps the option open.

ix's own shell already provides operator access into running VMs without a guest-side daemon. Connections survive reattach on the host side; the guest does not need to know. There is no reason to ship mosh, autossh, dropbear, or persistent tmux/screen reattach setups for connection survival, and modules should not reach for them. Terminal multiplexers are still useful for pane and tab management inside one session, which is a different concern; zellij ships in the base profile for that.

A VM in this repo is not necessarily a sealed appliance. Many of these images are SSH'd into and used as interactive dev machines, where a human reads `man`, browses `/etc`, tabs through Nushell completions, and reaches for `--help`. That is not universal: a pure backend service node (an internal Velocity proxy, a Postgres VM with no shell traffic) is fine to slim aggressively. Default toward keeping the on-host ergonomics that an operator would expect from a normal NixOS box; opt into closure-shrinking knobs (`documentation.enable = false`, `environment.noXlibs = true`, `programs.command-not-found.enable = false`) per image when the image is genuinely headless.

## Trust model

Assume the agent running inside a VM has root and a goal it is optimizing for. It can install packages, edit `/etc`, restart services, flip `networking.firewall.enable`, and overwrite anything reachable from inside the guest. What it does not have by default: host API credentials, the ix CLI's host-side authority, registry-write tokens, or any secret you do not mount in. Design follows from that asymmetry. Anything that must hold against a misbehaving in-VM process belongs outside the VM, where the agent cannot reach it.

Concrete shapes this principle produces:

- **Network policy.** In-VM `networking.firewall.*` is convenience for a cooperative guest. The guest can disable it, so it is not a containment boundary. When a port must stay closed against a rogue guest, the enforcement point is a separate router/gateway VM the agent has no shell on, or one of ix's group/internet primitives. Inline `networking.firewall.allowedTCPPorts` is still the right place to *declare* intent (co-located with the service that needs the port); the enforcing layer reads that intent from outside.
- **Secrets.** Long-lived credentials (API keys, registry-write tokens, cloud creds) stay on the host or in a sibling VM that does not share a kernel with the agent. Mount only the short-lived, narrowly-scoped material the agent needs for the current task.
- **Snapshot and rollback authority.** The operator owns snapshot/rollback. An agent that can `ix snapshot` its own VM can paper over destructive behavior, so that surface belongs on the host side.
- **Image and switch-source authority.** The agent should not be able to switch its own VM to an unsigned or unreviewed image, or rewrite the source the VM was built from. That capability lives with the operator.

The "VM networking" section below is one specialization of this principle. Apply the same lens whenever a new feature lands: "if a rogue agent in this VM tried to subvert this, where does the rule actually live?"

## VM networking

Networking policy lives in the image, not in ix. ix exposes two primitives: VM group membership (east-west, which VMs can reach each other) and internet ingress/egress on or off (north-south, per direction). Per-port filtering, L7 rules, WAF, rate limiting, and mTLS termination belong in the image's NixOS config (`networking.firewall.*`, services in front of the workload) or in a user-built gateway VM. Do not push port allowlists or L7 features into ix; the matching rule on the ix side is recorded in `ix/AGENTS.md` under "Architecture that must not drift". If a service needs only some ports exposed, declare it in the image with `networking.firewall.allowedTCPPorts` or front it with a gateway VM. Treat in-image firewall config as cooperative-guest intent per the trust model above: if the policy must hold against a rogue agent inside the same VM, the enforcement layer must live on a separate gateway/router VM the agent cannot reach.

## Port ownership

Default answer for same-protocol port conflicts is topology. In ix, put services that must keep the same public or natural port in separate fleet nodes/VMs. Two Minecraft Java servers can both bind `25565` cleanly when they are two nodes; squeezing them into one image makes the client-facing protocol fight the operating system.

Inside one image, every repo-owned module that binds a TCP or UDP socket must register `ix.networking.portClaims.<owner>` with `protocol`, `port`, `address`, and a short `description`. Add the claim next to the service bind settings or `networking.firewall.allowed*` declaration. Duplicate claims in the same `namespace` fail evaluation, which is the desired failure mode for accidental co-location.

Runtime fallback port allocation ("try `25565`, then pick whatever is free") is the wrong default for NixOS images. Firewall declarations, docs, client connection strings, health checks, and fleet plans need the chosen port before activation. Available-port probes also depend on service start order.

Use one of these shapes:

- Separate fleet node/VM when clients need the conventional port.
- Explicit static alternate port when same-image co-location is intentional.
- One frontend such as `nginx` or HAProxy on `80`/`443` with explicit upstream ports when clients only see HTTP(S).
- A real namespace boundary (NixOS container, systemd `PrivateNetwork` with veth wiring, or a separate bind IP) when services truly share one image. Set `ix.networking.portClaims.<owner>.namespace` to that namespace name so eval permits the duplicate.

## Registry access

Do not assume every `registry.ix.dev` image is public. The `ix` namespace is system-owned, so shared bootstrap refs such as `registry.ix.dev/ix/test-cluster-bootstrap:<tag>` are expected to be public. User images live under `registry.ix.dev/<username>/<image>:<tag>` and default to private; private images require the owner's auth and should behave like not-found for other users. When debugging image pulls, distinguish a public system bootstrap image from a user-owned private image before treating access as a registry outage.

The shared fleet bootstrap image is defined at `images/system/test-cluster-bootstrap`. It is an ordinary image that extends the repo base profile; keep source-switch tools such as `gnutar`, `zstd`, and `gzip` in `modules/profiles/base/` so any VM that has been switched once can be switched again. Build the bootstrap with `nix build .#test-cluster-bootstrap --print-out-paths --no-link`; upload it only with an admin ix profile, using an explicit system namespace ref such as `ix image push <archive>.tar registry.ix.dev/ix/test-cluster-bootstrap:<tag>`. TODO: replace full payload uploads with CAS/CDC-aware transfer for both bootstrap image publishing and `ix switch --source`, so routine updates send only changed chunks and then update the registry or switch input reference.

## Layout

```
flake.nix                                  # manifest: inputs + delegated outputs
.envrc, .githooks/pre-commit               # direnv sets core.hooksPath -> nix run .#lint
lib/
  default.nix                              # mkImage, discoverImages, ix.artifacts, helpers
  per-system.nix                           # per-system packages / apps / checks / formatter
  ix-platform.nix                          # target platform: EPYC Gen 5 (znver5), container mode
  ix-oci-layer.nix                         # OCI packaging, base profile
  minecraft-loader.nix                     # helper used by loader modules
  build-oci-image.sh                       # direct OCI archive builder
modules/
  default.nix                              # canonical module registry (attrset)
  profiles/base/                           # CLI tools + system-wide nushell config, on by default
  services/<name>.nix                      # opt-in service
  services/<family>/{default,...}.nix      # service family (runtime + plugins)
images/
  <category>/<name>/default.nix            # NixOS module
  <category>/<name>/versions.nix           # optional: per-version overlay modules
nix-rules/                                 # ast-grep lint rules
```

## Folder hierarchy

Repo folders follow the same "preserve the conceptual path" rule that the Rust/Nix style section applies to local bindings. When two or more siblings share a domain prefix, nest them under a parent directory whose name is the prefix. Prefer `packages/minecraft/nbt/` over `packages/minecraft-nbt/`, `packages/minecraft/rcon/` over `packages/minecraft-rcon/`, `modules/services/minecraft/paper.nix` over `modules/services/minecraft-paper.nix`. The dashed-flat form is a data-clump smell: it flattens a real hierarchy into a sibling-suffix list, and the next package in the same family then has to choose between joining the flat list or creating the parent everyone should have had from the start.

A few corollaries:

- Make the parent the moment a second sibling lands. The first `packages/foo-bar/` is fine in isolation; the second forces a rename, and the rename should happen in the same change that introduces the sibling.
- The package or module's published name (`Cargo.toml` `name = "minecraft-nbt"`, derivation `pname`, image tag) can stay dashed because that name is a foreign identifier under a different namespace. The repo path is what changes.
- Nested folders compose with the existing discovery and registry conventions. `images/<category>/<name>/` is already hierarchical; `packages/<family>/<name>/` and `modules/services/<family>/<name>.nix` follow the same shape so a reader scanning `packages/` sees families, not a long alphabetized list.
- Pure cross-domain tools that do not belong to a family stay at the top of their tree (`packages/llm-clippy/`, `packages/oci-image-builder/`). Do not invent an artificial parent for a single child.

When you encounter a legacy flat-dashed package or module while doing unrelated work, move it as part of that change if the rename is small and the call sites are inside the repo. Leave a follow-up only when the rename is genuinely larger than the work that uncovered it.

## Flake.nix style

`flake.nix` is the repo's handle. It should read like a manifest: a small inputs block and an `outputs` body that is mostly delegation. All logic lives in `./lib/` or behind discovery (`ix.discoverImages`). The goal is that someone landing on `flake.nix` cold can answer "what does this flake expose?" by skimming, not by parsing.

Do not put inside `flake.nix`:

- Raw fetched data artifact URLs. Human edits should land in the narrowest manifest or lock input the owning tool understands, then that tool should materialize hashes in generated catalogs, ecosystem lockfiles, or `flake.lock`. Repo-owned workflow tools are a separate category: a prebuilt executable such as the `ix` CLI may be a flake input when the intended bump path is `nix flake update`.
- App wrapper definitions (`writeNushellApplication { ... }` for `lint`, `update-mods`, `ix-fleet`, preset wrappers, etc.). Define them in a dedicated module under `./lib/` and reference them from `outputs` by name.
- Per-system `let`-bindings that compose many helpers. Push the composition into a single `mkOutputs system` function in `./lib/` and call it from `lib.genAttrs devSystems`.
- Image preset wiring, per-VM wrappers, or other scenario-specific command composition. Move it into the preset's own `default.nix` and import it once.

Target: `flake.nix` fits comfortably in a single screen and its body would look almost unsurprising as JSON. The cost of an inline helper today is the year-from-now untangle. Pay the structure cost up front.

Flake outputs stay on the standard schema: `packages`, `apps`, `checks`, `formatter`, `devShells`, `templates`, `overlays`, `nixosModules`, `lib`. Use `nixosModules` (plural, namespaced) for module exports. Do not add a flat top-level `modules` key: it is non-standard, not validated by `nix flake check`, and may not be discovered by downstream tooling.

## Adding an image

Drop a NixOS module at `images/<category>/<name>/default.nix`. That's it: discovery picks it up on the next eval and exposes `packages.<host>.<name>` for the supported dev systems. The derivation still targets `x86_64-linux`. No flake edits, no registry edits.

For a versioned image (multiple variants ship at once), add a `versions.nix` sibling:

```nix
{ lib, ... }:
let
  default = "1.21.11-fabric";
  variants = {
    "1.21.11-fabric" = {
      loader = "fabric";
      version = "1.21.11";
      mods = [ "fabric-api" "spark" ];
    };
  };
in
{
  inherit default;
}
// lib.mapAttrs (tag: { loader, version, mods }: {
  ix.image.tag = tag;
  services.minecraft = {
    inherit version;
    mods = lib.genAttrs mods (_: { });
    ${loader}.enable = true;
  };
}) variants
```

Discovery then exposes `<name>_<ver>` for each version key plus `<name>` as an alias for the `default` version.

## Adding a module

Drop the file at `modules/services/<name>.nix` (or `modules/profiles/<name>.nix`) and register it in `modules/default.nix`. Keep modules independent: declare `options`, gate everything behind `mkIf cfg.enable`, never import another module. The registry exists so option sets are visible to every image; modules stay inert until their `enable` flag is set.

## Adding a platform-wide default

Settings that should apply to every image without per-image opt-in have two homes. System-level posture (nftables, firewall, journald caps, `nix.settings`, `programs.nix-ld`, `systemd.coredump`, gc policy, znver5 host platform) lives in [`lib/ix-platform.nix`](lib/ix-platform.nix). The auto-enabled CLI baseline (debugging tools, source-switch utilities such as `gnutar`/`zstd`/`gzip`, the workspace shell wrapper, system-wide Nushell config) lives in [`modules/profiles/base/`](modules/profiles/base/), which [`lib/ix-oci-layer.nix`](lib/ix-oci-layer.nix) turns on for every image. Touch the platform module for system posture and the base profile for CLI ergonomics. Use `lib.mkDefault` on anything an unusual image might legitimately need to override, so that opt-out stays a one-liner.

Home Manager is used in this repo, but only in its NixOS-module form (`home-manager.nixosModules.home-manager`), not as a standalone workflow with `home-manager switch`. Operators connect as root, so there is no per-user dotfile divergence to manage; the reason HM is here is shell-tool integration. Tools like Nushell, atuin, zoxide, and starship expect XDG-shaped per-user config (`~/.config/<tool>/`), and their NixOS modules (where they exist) auto-wire init for bash/zsh/fish but rarely for Nushell. Hand-rolling `environment.etc` + `systemd.tmpfiles` symlinks for each tool compounds quickly; HM speaks XDG natively and exposes `programs.<tool>.enableNushellIntegration = true` across the ecosystem, which keeps the per-tool wiring a one-liner. Configure root's tooling through `home-manager.users.root.programs.<tool>`; reach for plain NixOS `programs.<tool>` when the tool is system-wide (sshd, sudo, nix-ld) or when its NixOS wrapper bakes the config into the binary so `~/.config/<tool>/` is just an XDG fallback the operator may use for overrides (`programs.neovim` is the current case: the wrapped `nvim` binary already loads its plugins, `customLuaRC`, and runtime files without anything in the home directory).

## Service families

When several modules vary along one axis (e.g. minecraft + fabric/paper/vanilla loaders), put the runtime in `modules/services/<name>/default.nix` and each variant in `modules/services/<name>/<variant>.nix`. The runtime declares a "slot" option; variants fill it. Wire each file into `modules/default.nix` as a separate registry entry.

For minecraft this means:
- `services.minecraft.serverJar` is the slot, declared by the runtime.
- Each loader module (fabric, folia, neoforge, paper, purpur, spigot, sponge, vanilla) sets `services.minecraft.serverJar` from its own URL/version options.
- Enabling a loader auto-enables the runtime via `mkDefault`.
- Enabling two loaders is a module-merge conflict → loud eval error.

## Typed NBT

Generated `.nbt`/`.snbt`/`.nbt.gz` files (datapack structures, scoreboard sidecars, anything that goes through `mkMinecraftNbtFormat`) live under `services.minecraft.serverFiles`, `services.minecraft.configFiles`, or `services.minecraft.datapacks.<name>.files`. The encoder needs the NBT type of every value because Minecraft cares whether a number is a `Short` or an `Int` and JSON erases that distinction. `ix.minecraft.nbt` exposes the constructors that tag a value with its intended NBT type (`byte`, `short`, `int`, `long`, `float`, `double`, `string`, `list`, `compound`, `byteArray`, `intArray`, `longArray`, `root`, `bool`).

When a file uses more than two or three NBT values, alias the constructor set as `tags` (plural) in a `let` binding and reference `tags.short 20`, `tags.compound { ... }`, and so on. This is the general "name a binding by what it holds" rule from the Rust/Nix style section: `ix.minecraft.nbt` is a collection of constructors, so its alias goes plural. For one-shot uses (a single NBT value in an otherwise non-NBT file), skip the alias and write `ix.minecraft.nbt.short 20` directly.

Bare Nix scalars still work where the implicit NBT type matches Minecraft's expectation: a string becomes `TAG_String`, a bool becomes `TAG_Byte` 0/1, an integer becomes `TAG_Int` or `TAG_Long` depending on width, a float becomes `TAG_Double`. Reach for the typed constructors when the implicit choice is wrong (most spawner and tile-entity NBT, where the field is a `Short` and an `Int` either gets silently misread or rejected).

## Mods

All mods go in `services.minecraft.mods`, keyed by Modrinth slug. Empty `{}` includes the jar with defaults. Attrsets with fields configure the mod.

```nix
services.minecraft.mods = {
  fabric-api = {};
  lithium = {};
  distanthorizons.maxRenderDistance = 512;
};
```

The `modCatalog` option maps slugs to locked artifact sources. Set by the image base (from `common.json`) and version overlays (from `<version>.json`), then enriched through `ix.artifacts.attachArtifactSources`, which wraps each catalog entry's `{ url, hash }` in a `pkgs.fetchurl` derivation. The runtime resolves every key in `mods` to that derivation's store path.

### Minecraft artifact catalog generation

The canonical Minecraft artifact catalog generator is `tools/update-mods.py`, exposed as `nix run .#update-mods`. For Fabric/NeoForge/Sponge mods, edit `images/games/minecraft/mods/manifest.json`, then run the app to regenerate `common.json`, the per-version lock catalogs, and the rich metadata catalog under `metadata/catalog.json`. For Paper plugins, edit `images/games/minecraft/plugins/paper/manifest.json`, then run `nix run .#update-mods -- --manifest images/games/minecraft/plugins/paper/manifest.json`. For one game version, pass `--version <version>`.

Modrinth-hosted entries should be listed by slug so the generator owns URL and hash selection. When a Bukkit plugin's runtime name differs from the slug, set `pluginName` in the manifest so PlugMan reloads use the correct name. Non-Modrinth or hand-picked artifacts belong in the manifest as a small object with the slug and URL; the generated catalog is where `{ url, hash }` belongs. Do not hand-edit the generated JSON except to inspect a diff before committing.

Use manifest `searches` for broad agent-queryable indexes such as popular Fabric mods or Paper-compatible server plugins. Search results enrich `metadata/catalog.json` with descriptions, project pages, icons, gallery images, links, selected version files, and dependency metadata without adding every discovered artifact to `services.minecraft.mods` or `services.minecraft.plugins`. Keep the per-version lock catalogs curated; they are the install set that Nix consumes.

### Mod modules

Mods with config files get a NixOS module at `modules/services/minecraft/mods/<name>.nix`. The module activates when `services.minecraft.mods.<slug>` is present, reads the user's attrset (with defaults), and generates `configFiles`.

```nix
# modules/services/minecraft/mods/distant-horizons.nix
{ config, lib, ... }:
let
  modCfg = config.services.minecraft.mods.distanthorizons or null;
  defaults = { serverSideLodGeneration = true; maxRenderDistance = 256; };
  merged = defaults // (if modCfg == null then {} else modCfg);
in
{
  config = lib.mkIf (modCfg != null) {
    services.minecraft.configFiles."DistantHorizons.toml" = {
      server = { inherit (merged) serverSideLodGeneration maxRenderDistance; };
    };
  };
}
```

Mods without config (lithium, krypton, chunky) do not need a module. The slug in `mods` is sufficient. Register mod modules in `modules/default.nix` under `minecraft.mods.<name>`.

### Minecraft player access

Whitelist and operator files are derived from `services.minecraft.players`. Put each player's UUID and optional display name there, then set `whitelist = true` and/or `operator.enable = true` on the player. Use `services.minecraft.whitelist.enable` for `server.properties` whitelist enforcement. Presets and examples must not hand-author `ops.json` or `whitelist.json`; those are mutable server-state files, so ix reconciles a Nix-managed subset at runtime and preserves entries that were not previously managed by Nix.

### Config file format inference

`services.minecraft.properties` writes `server.properties`, and `services.minecraft.worlds.<name>.generator` renders Bukkit world generator bindings into `bukkit.yml`. Use those first-class options in images, presets, and examples; keep `services.minecraft.bukkit` and `serverFiles` as escape hatches for less common root files. `configFiles` keys are relative paths under `config/`. The serialization format is inferred from the file extension: `.toml`, `.json`, `.yaml`/`.yml`, `.properties`. Values are plain Nix attrsets. Mod modules never import `pkgs.formats` directly.

```nix
services.minecraft.properties.motd = "ix-powered Minecraft";
services.minecraft.worlds.factions.generator = "TerraformGenerator";
services.minecraft.configFiles."SomeMod.toml" = { section.key = "value"; };
services.minecraft.configFiles."other.yml" = { setting = true; };
```

### Writable vs read-only configs

Config files are symlinked to the Nix store (read-only). Some mods write to their config at runtime. If a mod needs a writable config, the mod module should copy instead of symlink. This is not yet implemented but is a known gap (see nix-minecraft's `files` vs `symlinks` pattern for reference).

## Cross-cutting helpers (`specialArgs.ix`)

Helpers shared across modules go in `lib/` and are exposed to every module through `specialArgs.ix`. Modules consume them as ordinary module args:

```nix
# modules/services/minecraft/paper.nix
{ ix, config, lib, ... }:
ix.mkMinecraftLoader {
  inherit config lib;
  name = "paper";
  dropinDir = "plugins";
  extraOptions = { /* loader-specific options */ };
}
```

This is the only way modules in this repo reach helpers in `lib/`. **Never** use `../` or `../../` paths to climb out of `modules/`. Relative-path imports between modules and lib break the abstraction (modules become coupled to lib's filesystem layout) and make module files harder to relocate. If a helper needs to be shared, expose it through `specialArgs.ix`. The helper bundle is also part of the public flake `lib` output, so external users get the same surface.

## Doc-comments

Public helpers exposed through the flake `lib` output and through `specialArgs.ix` use `/** ... */` doc-comments placed immediately before the binding (RFC 0145). CommonMark inside. Document the API: what the helper does, the shape of its arguments, and the shape of its return. Implementation-only `#` comments stay for the "why" notes the rest of this document covers; doc-comments are the API surface. When adding a new helper next to a file with a single top-of-file block comment, lift the relevant prose into per-binding doc-comments.

```nix
/**
Build one self-contained OCI archive from a list of NixOS modules.

Runs `nixpkgs.lib.nixosSystem` over the platform config, OCI packaging, the
module registry, and any caller modules, then streams the toplevel into an
OCI archive. Returns the archive derivation.
*/
mkImage = args: (evalImageConfig args).ix.build.ociImage;
```

## Module conventions

- Modules declare options and config. They never `imports` another module.
- Top-level options live under `services.<name>` for services, `ix.profiles.<name>` for profiles. Never reach into another module's namespace.
- Everything in `config` is wrapped in `mkIf cfg.enable`. The base profile is the only exception: it ships an enable flag so users can opt out.
- Module options take strings, paths, or packages. Per-version data goes in `versions.nix`; a module file should accept plain values, never a factory wrapper.
- Public option names should describe the user's domain, not the storage mechanism. Prefer `services.minecraft.plugins` for Bukkit-family plugins and `services.minecraft.mods` for Fabric/NeoForge/Sponge mods; avoid vague plumbing names such as `extraJars` or `dropins` unless the storage mechanism is itself the concept.
- Cross-module helpers come from `specialArgs.ix`. No `..` paths.
- Modules that render a structured config file expose typed settings backed by `pkgs.formats.*` with a freeform submodule (RFC 0042). The repo's `services.minecraft.configFiles` slot is the canonical pattern: keys are relative file paths, values are plain attrsets, and the format is inferred from the extension. Do not introduce stringly `extraConfig` options on new modules; concatenating strings can't merge same-key assignments, defeats `mkDefault`/`mkForce`, and makes values uninspectable.

## Path boundaries

Relative-up paths (`../`, `../../`, etc.) are usually an anti-pattern in tracked Nix code. They couple a file to a caller's current location instead of to the repo API boundary. Prefer named package sets, flake inputs, module options, or helpers exposed through `specialArgs.ix`. If a file needs something outside its directory tree, first ask which boundary should own that dependency and expose it there.

Relative paths to children or siblings inside the same package/module directory are fine. Relative-up paths are acceptable only when they are local, standard for the tool or ecosystem, and not reaching across a repo layer. The smell is climbing upward to reach across layers such as `images/` -> `packages/`, `modules/` -> `lib/`, or presets -> repo internals.

## Plugin conventions

Bukkit-family loaders (Paper, Folia, Purpur, Spigot) use `services.minecraft.plugins`. Empty `{}` resolves a pinned plugin by slug from `pluginCatalog`; an attrset with `src` installs a local or private plugin jar; other attrset fields are reserved for plugin-specific modules such as `services.minecraft.plugins.simple-voice-chat.port`. The repo's plugin and mod catalogs (`ix.lib.artifacts.minecraft.*`, the generated JSON catalogs under `images/games/minecraft/mods/` and `images/games/minecraft/plugins/`) are the shared surface that presets, examples, and images consume. Presets and examples must not inline plugin or mod URLs and hashes; see the "Presets never own artifact data" rule under Image preset conventions.

When a plugin has required companion config, model that as a module activated by the plugin slug instead of making every consumer hand-write sidecar files. For example, `services.minecraft.plugins.terraformgenerator.worlds = [ "factions" "factions_nether" "factions_the_end" ];` should contribute matching `services.minecraft.worlds.<name>.generator` defaults, which the runtime renders to `bukkit.yml`.

Fabric/NeoForge/Sponge-style artifacts stay in `services.minecraft.mods`. Keep mod and plugin catalogs near the image/module artifact plumbing, not in preset fleets. Preset fleets should read like intent: choose a server, select catalog plugins/mods by slug, and show local/private artifacts only when that is the point of the preset.

Paper plugin artifacts are generated from `images/games/minecraft/plugins/paper/manifest.json` into `ix.artifacts.minecraft.paperPluginCatalogs.<version>`. Loader modules seed `services.minecraft.pluginCatalog` from that versioned catalog, with `ix.artifacts.minecraft.paperPluginCatalog` kept as the current default alias. Add shared Paper plugins through the manifest and generator, not by hand-editing `lib/default.nix`.

## Image conventions

- Images set `ix.image.name`. They may set `ix.image.tag` (defaults to `latest`, or comes from `versions.nix`).
- Images compose by enabling services and adding packages. They do not declare options. They do not `imports` anything.
- Images stay version-agnostic when they have a `versions.nix`. The base file is what every variant shares; per-version data lives in the overlay.
- Use a single `services.<name>` block per service. Nest sub-options inside attrsets instead of writing scattered dotted assignments. Prefer `services.minecraft = { plugins = { luckperms = { }; claude-code-scoreboard = { ... }; }; };` over separate `services.minecraft.plugins.luckperms = { };` lines in presets.
- Options that are redundant with their namespace should be shortened. `services.minecraft.folia.version`, not `services.minecraft.folia.minecraftVersion`.
- Images should consume repo-local packages through `ix.packages`, not by importing `../../..` paths into `packages/`. Keep source-path ownership in the package set, following nixpkgs' `callPackage`/package-set style: modules and images choose package values, while package definitions own their filesystem layout.

## DRY user-facing options

Every fact a user states in `config` should appear once. Apply this strictly when designing or extending a module, library, or helper: if a typical preset sets `services.foo.version = "1.2.3"` and then also writes `services.foo.src = artifacts.foo."1.2.3"` and `services.foo.modCatalog = catalogs."1.2.3"`, the API is wrong. Restructure so the version drives the derived defaults and the preset only mentions `1.2.3` once.

Three failure modes that justify a restructure:

- **Duplicated identifiers.** The same string appears in two or more option assignments in a typical config. Declare one canonical option (the version, hostname, slug, region) and have the rest default off it. Cross-option defaults (option B reads option A) are how you express derivation; use `defaultText` so the manual still shows the intent.
- **Per-call-site defaults.** Every caller writes the same `src = artifacts.foo.X;` line. Move the default onto the option (`default = artifacts.foo.X;`) so callers only override the exception. The library is where reachable defaults live; the preset chooses among them by name.
- **Cargo-cult options.** A module declares `services.foo.bar.flavor` but no `config` block ever reads it. Delete the option. Forcing presets to set a value that nothing consumes is worse than not having the option, because it tricks readers into thinking the value matters.

Presets are the API's specification. Write the preset first; if a single intent ("use Paper 1.21.11") takes more than one line, the option set is too wide. A verbose preset is a bug in the module's API, not in the preset. The "Presets never own artifact data" rule below is the library-side complement: keep artifact data in `ix.lib.*` so presets can stay this short.

The repo has no external consumers, so renaming or collapsing options is free. Pay the migration cost (callers, tests, docs) in the same change.

## Image preset conventions

Image presets live under `images/presets/`. They are teaching material, not just tests: each preset should be a runnable image or fleet shape that composes the repo's normal modules, packages, and artifact catalogs by name. Add short comments for ix-specific ideas that a first-time reader will not infer from Nix alone: `deployment.switch.overrideInputs`, remote switch builds, fleet defaults, hot-reload behavior, and why an image name or tag is set.

Examples and presets should make the backing API look good. If an example needs to repeat obvious safe defaults, conservative service settings, artifact plumbing, or boilerplate just to be production-shaped, fix the module/helper defaults instead and keep the consumer minimal. The example should show intent; the library should carry the policy.

### Presets never own artifact data

Presets must not inline URLs, hashes, or pinned version strings for fetched artifacts. Mod jars, plugin jars, server jars, datasets, JDKs, and source-fetched packages all live in the repo's library surface (`ix.lib.artifacts.*`, `ix.packages`, module options, generated catalogs under `images/`), and presets consume them by name. If a preset needs an artifact the library does not expose yet, extend the library first: add the slug to the relevant catalog and regenerate it with `nix run .#update-mods`, add a new entry to `ix.lib.artifacts`, or grow the relevant module option. Then point the preset at the named surface. Presets are consumer tests for whether the library is sufficiently specified. A missing entry is a gap in the repo; fix it in the library so every consumer benefits.

In-repo presets that exercise this repo's library, modules, fleets, or pinned artifacts should be exposed from the root `flake.nix` as `apps`/`packages` and share the root `flake.lock`. Keep the actual fleet or image value in the preset's `default.nix` so tests and root outputs can import it. Do not add a nested preset `flake.lock` that pins this same repo; it will drift from the API under test.

Use a standalone template or `examples/<name>` flake only when it is intentionally a downstream consumer. In that case its inputs should look like a real external user's inputs (`github:indexable-inc/index`, registry refs, etc.), not `path:../..` or `git+file:../..` backedges into the parent checkout. Do not add template-local artifact inputs when the root flake already exposes the locked artifact through `ix.lib.artifacts`.

In fleet presets, `ix.image.name` usually defaults to the node name. Set it only when the replacement image should be named differently. Set `ix.image.tag` when the default `latest` would make plans or registry destinations ambiguous.

Comments should explain why a line exists, not restate Nix syntax. Prefer comments that answer "why is this needed in an ix fleet?" over comments that paraphrase the option name.

Example docs and example flakes assume the reader already has the `ix` CLI.
They should not include generic `Use` sections or wrapper outputs for
`nix run .#plan`, `nix run .#up`, `nix run .#switch`, or routine
`ix shell ... journalctl` inspection. Every standalone example README should
include a short `Run` section that shows `ix up` as the whole lifecycle command.
Do not explain what happens behind that command. Do not spell out lower level
image plumbing such as `nix build`, image pushes, or `ix new` unless that
plumbing is the point of the example. Example `flake.nix` files expose the
values ix needs plus any local package the example owns. Leave
`plan`/`diff`/`up`/`switch`/`replace` apps and matching wrapper packages out of
standalone consumer examples. Keep examples focused on the shape being taught:
files, modules, service settings, data paths, and the specific operational
caveats that are unusual for that example.

Default to Rust for repo-owned tools that parse structured data, reconcile mutable state, stream archives, move large byte ranges, implement nontrivial CLIs, or sit in build/runtime hot paths. In practice, the vast majority of new first-party tooling with real logic should be Rust with normal source files, Cargo metadata, tests where useful, and Nix packaging. Shell is fine for small orchestration around existing programs, and Python is fine for low-volume generators or ecosystem-heavy tasks such as catalog updates, but do not leave stateful reconcilers, performance-sensitive builders, or runtime helpers in Python merely because it is quick to prototype.

Prefer structured encoders at the boundary that owns the data. For JSON, YAML, TOML, properties files, and test fixtures, use Nix values with `pkgs.formats.*` or a language-native serializer, then copy or link the generated file from the shell step. Keep shell focused on orchestration: arranging directories, invoking tools, and checking outputs. This keeps escaping, ordering, and encoding consistent, and makes the data shape reviewable as Nix or typed source instead of as heredoc text.

When reasoning about build performance, assume package dependencies are already cached unless the question is specifically about bootstrap or cold-cache behavior. Treat Rust crates, Python dependencies, and other toolchain inputs like nixpkgs does: they are expected to be prebuilt/substituted in normal use, so benchmark the repo-owned derivation or image assembly path after dependencies are present.

To measure build time, prefer `time -p nix build .#<attr> --rebuild --print-out-paths --no-link` for the derivation under investigation. Use `--log-format internal-json -v` when you need structured Nix events, and `-L` when builder logs matter. For image assembly specifically, compare cached top-level rebuilds so dependency fetching and unrelated invalidations do not hide the packaging cost.

## Dependency intake

Every external artifact enters the repo through a narrow intake surface: an ecosystem lockfile, a fixed-output Nix fetcher, generated catalog data, or a pinned image/source reference. The same review rule applies to all of them. Commit the exact artifact identity, make the selected version old enough for the ecosystem to have reacted, and keep enough metadata for a later reader to answer what changed without re-running the resolver.

Lockfiles and SRI hashes record what was selected. They do not prove the selected artifact was mature when the lock was written. A lock update created 3 minutes after an upstream publish can faithfully pin a compromised tarball. Queue time before merge helps only when the selected artifacts are rechecked near merge time or when the updater opens the PR after a minimum-age gate has already passed. Default to 24 hours before merging routine third-party version bumps. A 12-hour delay is acceptable for low-blast-radius dev tooling when the maintainer explicitly chooses speed; emergency security fixes can move faster, but the commit or PR body should name the advisory and the reason the age gate was bypassed.

Apply this to every ecosystem this repo uses:

- **Nix fetched artifacts and generated catalogs.** Keep URL, version, upstream project page when available, and SRI hash together at the point of use. For generated catalogs such as Minecraft mods/plugins, use the generator so the lock data and metadata move in the same diff. Treat brand-new upstream releases, changed download hosts, and git/tarball indirections as review events.
- **Rust, Python, Java, JavaScript, and Bun support projects.** Use the ecosystem lockfile as the dependency graph owner (`Cargo.lock`, `uv.lock`, `gradle.lockfile` plus `gradle/verification-metadata.xml`, `package-lock.json`, `pnpm-lock.yaml`, or `bun.lock`). Dependency updates should be isolated from source refactors so the resolver diff is reviewable.
- **OCI images and source refs.** Prefer immutable digests or reviewed tags that resolve to a known digest. Floating tags such as `latest` are acceptable only for local experiments or examples where freshness is the point.
- **Install/build scripts.** Any dependency that runs code during install, code generation, native compilation, or Gradle/Maven plugin resolution is a higher-risk dependency. Keep the allowlist small and committed. A new script runner is a review blocker even if the version is old enough.

Use the ecosystem's normal project shape before inventing local scaffolding. Java support projects should be Maven or Gradle projects with a `pom.xml`/build file, `src/main/java`, and resources; build them from Nix with `maven.buildMavenPackage` or the corresponding standard builder. Do not generate source files from Nix heredocs, vendor fake API stubs, or hand-roll classpaths when a normal build tool dependency is available.

Web support projects should use ordinary frontend project structure too. Prefer TypeScript over JavaScript, split real UI into components/modules once a single file stops being clearer, and keep strict typechecking and ESLint in the default build path. For Svelte/Vite projects, `npm run build` should run `svelte-check`, ESLint, and the production bundle so `buildNpmPackage` enforces the same checks as local development. Use SvelteKit only when the project needs routes, server-side loading, endpoint handlers, or an app runtime; static status pages can stay Svelte/Vite.

For new repo-owned JavaScript projects, use `pnpm` 11 or newer when the package manager is open. Keep supply-chain hardening in `pnpm-workspace.yaml`: `minimumReleaseAge: 1440` or stricter, `blockExoticSubdeps: true`, `strictDepBuilds: true`, and an explicit `allowBuilds` map for dependency install scripts. Use `pnpm approve-builds` to review the few packages that need `preinstall`, `install`, or `postinstall`; commit both approvals and denials so the next install fails loudly when a new package tries to run code. Never set `dangerouslyAllowAllBuilds` in repo-owned projects. For existing `package-lock.json` projects, keep `npm ci`/`pkgs.importNpmLock` style reproducibility, and treat new install scripts or transitive git/tarball sources as review blockers.

Publishing and deployment workflows must keep untrusted pull-request execution separate from privileged release state. Avoid `pull_request_target` and `workflow_run` for jobs that check out or run PR code. If a workflow needs those events for labels, comments, or maintainer-only metadata, keep it metadata-only with read-only permissions, no dependency cache writes, and no package-manager install from the PR head. Main/tag release jobs should restore caches only from trusted refs, mint registry publish credentials only after the reviewed ref is selected, and never pass publish tokens through caches, artifacts, or logs. Trusted publishing is still a credential boundary; it does not make a poisoned build cache safe.

Do not hide real source files inside Nix strings just to keep the file count small. If a preset needs Java, scripts, config templates, or assets, put them in ordinary files with normal paths and keep the Nix derivation as the build recipe. Inline generated files are acceptable only for tiny machine-owned glue where reading a separate file would be worse.

At the same time, do not spray files around without a boundary. Group support code under a named subdirectory with a small `default.nix`, source files, and assets it needs. Reusable server code, plugins, and other composable artifacts belong under `packages/<family>/...`; image directories should compose those artifacts, not own unrelated build projects.

For self-contained support projects, filter at the project boundary instead of listing every source file. Prefer `lib.fileset.intersection (lib.fileset.gitTracked ./.) ./.` in the project-local `default.nix` so new tracked files under `src/`, `resources/`, Gradle metadata, or similar project-owned paths are included automatically while untracked build caches stay out of the store. Use explicit file lists only when the derivation intentionally consumes a small cross-cutting subset.

## Audio data

Prefer `WAV`, stereo PCM, `44 kHz`, `float32` for repo-owned audio data by default. Choose another lossless format or sample rate only when a downstream tool, platform, or source asset requires it, and record that reason next to the data or derivation.

## Artifact inputs

Fetched artifacts (mod jars, server jars, plugins, source trees) belong to the nearest owner that can update them mechanically. Prefer ecosystem lockfiles (`Cargo.lock`, `uv.lock`, Gradle metadata, npm lockfiles), repo manifests consumed by an updater (`images/games/minecraft/mods/manifest.json`), or `flake.lock` when the artifact is a flake-managed tool pin. The human workflow is to edit the manifest or source requirement, run the updater, and commit the generated hash-bearing output with the source change.

The `flake.nix` inputs list is reserved for things that genuinely participate in the flake graph: `nixpkgs`, tooling flakes, and workflow tool binaries whose practical update surface is `flake.lock`. The precompiled `ix` CLI is in this bucket. Prefer immutable release URLs, tag URLs, or an upstream flake so an existing lock keeps building after a newer binary is published. A `latest` URL that overwrites bytes under the same path is a publishing problem; fix or version the endpoint before moving that hash into package code.

Pick the most specific `pkgs.*` fetcher for the source. `pkgs.fetchurl` is right for opaque single-file downloads (jars, zips, tarballs hosted at a stable URL). For source trees, prefer the upstream-specific fetcher so the derivation captures the right metadata and so future bumps go through one well-known field: `pkgs.fetchFromGitHub`, `pkgs.fetchFromGitLab`, `pkgs.fetchFromForgejo`, and friends for forge tarballs; `pkgs.fetchgit` for raw git refs; `pkgs.fetchzip` for archives that should be unpacked; `pkgs.fetchMavenArtifact`, `pkgs.fetchNpmDeps`, `pkgs.fetchCrate` for ecosystem artifacts. Do not use `builtins.fetchurl`, `builtins.fetchTarball`, `builtins.fetchGit`, or `builtins.fetchTree` in tracked Nix files: those run on eval, are not fixed-output derivations, do not substitute from a binary cache, and are banned in nixpkgs. The `pkgs.*` fetchers are (optionally) fixed-output derivations and only fetch at build time.

Earlier versions of this repo tracked every mod jar as a non-flake `inputs.artifact-*` URL so that `flake.lock` would own each `narHash`. That made `flake.nix` unreadable and centralized nothing useful: each entry still had to be edited individually, and the lock file became a churn-heavy diff for every routine mod bump. Prefer a small manifest plus generated catalog; `nix run .#update-mods` regenerates URLs and hashes together.

Tracked Nix files must not contain `lib.fakeHash`, `lib.fakeSha256`, `lib.fakeSha512`, or placeholder hashes such as `sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=`. Use the repo updater, ecosystem lock command, `nix flake update`, or a checked prefetch command to materialize the real SRI before committing. A hand-maintained inline hash is a last resort for sources that cannot fit a better owner; leave the reason near the fetcher.

## Target platform

All images run on AMD EPYC Gen 5 (Turin, Zen 5). `lib/ix-platform.nix` sets `nixpkgs.hostPlatform.gcc.arch = "znver5"` and `tune = "znver5"`, which propagates `-march=znver5 -mtune=znver5` to every package in the closure. This enables AVX-512, VNNI, and other Zen 5 instructions across the board.

Because the arch differs from the nixpkgs binary cache (generic x86_64), every package builds from source. This is intentional: these images run on known hardware and the build cost is paid once.

When adding new modules or packages, do not override compiler flags per-package. The base layer handles it globally. If a package needs arch-specific tuning beyond compiler flags (e.g. PostgreSQL `huge_pages`, JVM `-XX:+UseAVX`), set those in the module.

## Nix philosophy

- **Single source of truth.** `modules/default.nix` is the only place modules are listed; `lib.collect` derives the flat list from the nested attrset. Versions live next to the image in `versions.nix`, not in `flake.nix`. Hashes live next to URLs.
- **Composition lives here, primitives live in `ix`.** The closed-source `ix` CLI and SDK own low-level VM operations: create, shell, snapshot, replace, registry push. Compositional work (fleet planning, multi-node ordering, health-check schemas, lifecycle wrappers, test runners, batch tooling) lives in this repo and consumes those primitives through [`packages/ix-fleet`](packages/ix-fleet/) and the Nix-rendered plan in [`lib/fleet.nix`](lib/fleet.nix). When a missing workflow tempts a CLI extension, the default is a composition layer here that calls the existing primitive. The line stays clean as long as per-workflow features stay out of `ix`.
- **No backwards compat.** This repo is young and has no external consumers. Rename freely, change signatures, delete dead code. No shims, no aliases, no `// removed` comments, no feature flags for the old way. Update callers in the same change.
- **Auto-discover, don't enumerate.** `flake.nix` walks `images/`. Adding an image is `mkdir + edit one file`. Hand-wired registries rot.
- **Aggregate views live in `lib/`, not in callers.** When a workflow needs "all X across the repo" (health checks, port claims, listening ports, declared secrets), add a small `lib/` helper that walks the canonical source directory and exposes a plain-data attrset under `lib.<name>`. Don't hand-list inputs in `per-system.nix`, duplicate the walk inside an app, or bury the aggregation in a one-off script: the lib version is reachable by other Nix code, by `jq`, and by any wrapper at once.
- **Prefer `nix eval --json` plus the local shell over bespoke wrappers.** Once a value is exposed as `lib.<name>`, `nix eval .#lib.<name> --json | from json` (Nushell) or `| jq` already turns it into a table, a filter, or a one-off transform. That covers most "let me look at X" needs without adding code. Only add an `apps.<name>` wrapper when the formatting is non-trivial (custom layout, multi-source join, follow-up actions, output that has to survive being passed to another tool) and document the wrapper as a shortcut over the same eval, not as the only way in. A new app that runs `nix eval … | from json | <one-liner>` is the form to delete, not the form to add.
- **DRY at the data layer, not the abstraction layer.** `inherit (pkgs) ...` over a wrapper helper. `lib.collect` over a parallel list. Don't introduce a function unless it has at least two callers; the minecraft-loader helper qualifies because eight loaders share the same shape.
- **Comments explain why, not what.** Headers say what each file is for and what's load-bearing (e.g. `maxLayers = 67` with the registry-cap rationale, base profile auto-enabled). Don't restate what the code obviously does.
- **Trust module merging.** Layer per-version overlays via the module system, not by passing args to factory functions. Service families (runtime + variants) compose through option slots, not through wrappers.
- **Pure eval.** No `builtins.currentSystem`, no `builtins.getEnv`, no `<nixpkgs>` channel refs, no `path:` flake refs. Every input flows through `flake.nix`.
- **IFD is allowed when it removes a fake Nix layer.** Import From Derivation is acceptable for builders that first need another tool to reveal the real build graph, such as Cargo unit graphs. Keep the IFD boundary explicit and inspectable by exposing the generated artifact. The assumption is that this repo targets Nix implementations where IFD does not pin the main evaluator thread long-term: either mainline Nix gets good enough here, or evaluators such as snix keep advancing their existing support. Do not contort dynamic tool output into hand-maintained Nix data just to avoid IFD.
- **Batch IFDs into one bigger derivation.** When IFD is justified, prefer one derivation that discovers many things over many derivations that each discover one thing. On mainline non-vendored Nix the evaluator walks IFDs serially on a single thread, so N per-target list drvs cost N eval barriers even when the underlying builds could have run in parallel. A single manifest drv that depends on every input pays one barrier and lets the inner work fan out under that one build. `nix-cargo-unit`'s `testManifestDrv` is the worked example: one IFD enumerates every test binary's `#[test]`s for the whole workspace instead of one IFD per binary. The rule is independent of the per-IFD cost — even cheap discovery serializes if you split it.
- **Strict, named failures.** `lib.assertMsg` over bare `assert`. Required options have no default so misuse fails at eval with the option name. Two loaders enabled → module-merge conflict, not silently-last-wins. The `ix.errors.*` helpers in [`lib/errors.nix`](lib/errors.nix) wrap the common shapes (enum validation, mutex of bool flags, attrset lookup, missing flake input) with messages that name the option and list the valid alternatives, so a typo or a forgotten input lands with a fixable error instead of an `attribute missing` from somewhere deep in eval.
- **Test useful invariants.** Eval tests should protect behavior that can regress across module boundaries, generated units, fleet rendering, artifact wiring, or security/runtime contracts. Do not assert facts that are already obvious from the same literal config being imported, such as exact preset node names, hand-copied package allowlists from auto-discovery, or every field in a preset's own `server.properties`. Flake evaluation already catches syntax and missing-output failures; tests should add signal beyond that.

## Nix practices to tighten

These are current repo habits that should not become defaults. When touching nearby code, improve the pattern in the same change. If the cleanup is wider than the task, file a narrow issue.

- **Typed module interfaces.** Avoid `types.attrs`, `types.attrsOf types.attrs`, and `types.anything` for domain data. Use `types.submodule`, `attrsOf (submodule ...)`, `types.enum`, `types.oneOf`, `types.nullOr`, or a `pkgs.formats.*.type` that matches the file being generated. Keep broad attrs only at true foreign-format boundaries, and name that boundary in the option description.
- **Filtered local sources.** Do not default to broad `src = ./project` or `src = ./site` inputs. Use `lib.fileset.toSource` with the smallest useful file set, usually `lib.fileset.intersection (lib.fileset.gitTracked ./.) (lib.fileset.unions [ ... ])`, or `lib.sources.cleanSourceWith` when a predicate is clearer. This avoids copying irrelevant files or secrets into the store and prevents rebuilds from unrelated local changes.
- **Executable paths.** Prefer `lib.getExe pkg` when the package's `meta.mainProgram` is correct, and `lib.getExe' pkg "program"` when the executable name is intentionally explicit. Add `meta.mainProgram` to repo packages that install a primary binary. Avoid scattering `"${pkg}/bin/foo"` through systemd units, apps, tests, and scripts unless there is no package value to pass around.
- **Checked builders.** Repo helpers that generate commands should run the strongest practical build-time validation by default, and callers should reuse those helpers instead of open-coding ad hoc wrappers. Examples: `ix.writeNushellApplication` runs Nu diagnostics, and `ix.writePythonApplication` runs basedpyright with required types. Keep the policy generic: add or extend one DRY helper when a language/tool needs validation, then use it everywhere.
- **Fix the improper layer.** If an idiomatic cleanup exposes that the underlying helper, package, or tool is not proper, fix that layer in the same change instead of weakening validation or adding a local workaround. Only narrow the check when the stricter mode exposes unrelated legacy debt, and leave the tool on the strongest mode it currently satisfies.
- **Shell applications.** Use `ix.writeNushellApplication pkgs { ... }` for generated commands that call other programs, so runtime dependencies are explicit and Nu syntax is checked during the build. Do not use `writeShellApplication` or `writeShellScriptBin` in tracked Nix files. Tiny `writeShellScript` glue is acceptable only when the output is not a user-facing command.
- **Nushell wrappers.** Keep wrappers real Nu, not Bash hidden inside a Nu string. Use `def main [...args]`, structured values, lists with `...$args`, and `builtins.toJSON` for Nix-to-Nu literals. The wrapper helper must prepend declared runtime inputs while preserving the ambient `PATH`; fleet/app wrappers may need commands supplied by the caller, such as a freshly patched `ix` binary.
- **Scripts.** Prefer Nushell (`.nu`) over Bash for new non-trivial repo scripts, especially when the script parses JSON, builds structured output, or has enough branching that shell quoting becomes load-bearing. Package these scripts with `ix.writeNushellApplication pkgs { ... }` so runtime dependencies are explicit and Nu syntax is checked during the build. Bash is fine for tiny POSIX-style wrappers.
- **devShells.** Default to no devShell. Most of what people reach for one to do can be done without one. Env vars and PATH-bound tools belong in `.envrc` (direnv handles its own bootstrapping; do not wrap direnv in a `shellHook`). Editor-only tools (LSPs, formatters) belong in your editor config. A package that is genuinely required at build time belongs in that package's `nativeBuildInputs`. Per-package shells already exist: `nix develop nixpkgs#hello` enters `pkgs.hello`'s build environment, and `nix develop .#<package>` does the same for repo packages. The only place a `devShells.default` carries its weight is the unified-shell case: when you're regularly working across many of the repo's own packages and want one entry point, build it with `inputsFrom = [ pkg1 pkg2 ... ]` instead of accumulating a junk drawer in `mkShell.packages`.
- **Pre-commit.** Do not depend on the `cachix/git-hooks.nix` framework when a one-line hook does the job. Use `.githooks/pre-commit` (chmod +x) that runs `nix flake check` or `nix run .#lint`, and have `.envrc` set `GIT_CONFIG_COUNT=1 / GIT_CONFIG_KEY_0=core.hooksPath / GIT_CONFIG_VALUE_0=./.githooks`. The lint app is the single source of truth; flake checks reuse it.

## Nix style (ast-grep enforced)

Run `nix run .#lint` before committing. It runs `nixfmt`, `statix`, `deadnix`, and the repo's ast-grep rules. Hard rules:

- No `with pkgs;` or `with lib;`. Use `inherit (pkgs) ...` or `lib.foo` directly.
- No `rec { }`. Use `let ... in` or `final/prev` instead.
- No `mkForce`. Resolve conflicts with priority composition or fix the module boundary.
- No `lib.recursiveUpdate`. Build the attrset in one place or use `lib.mkMerge`.
- No repeated parent keys in the same attrset. Group related assignments under one parent, e.g. `services.minecraft = { ...; };` or `environment.etc = { ...; };`, instead of several `services.minecraft.foo = ...;` lines in the same attrset.
- Prefer `inherit (source) name;` for direct field copies when the local name is the same. Avoid `name = source.name;` unless the assignment is clearer because it transforms or documents a boundary.
- No `builtins.currentSystem`, `builtins.getEnv`, `<nixpkgs>`, or `path:` flake refs.
- No `(import ./foo.nix)` inside `imports = [ ... ]`. NixOS auto-imports paths.
- No `..` paths inside `modules/`. Cross-cutting helpers come through `specialArgs.ix`.
- No `writeShellApplication` or `writeShellScriptBin`. Use `ix.writeNushellApplication pkgs { ... }` for user-facing commands and orchestrators.
- No bare `assert cond;`. Use `assert lib.assertMsg cond "why";`.
- No unused bindings. Use `_` for intentionally unused lambda arguments, remove unused module args, and run `deadnix --fail --no-lambda-pattern-names .` through `nix run .#lint`.
- `strictDeps = true` on every `mkDerivation`. `__structuredAttrs` is the nixpkgs default; do not set it explicitly.
- No raw fetched data artifact URLs in `flake.nix` inputs. Fetched assets (jars, plugins, server tarballs, source trees) should be owned by ecosystem lockfiles, repo update manifests, generated catalogs, or a narrow `pkgs.*` fetcher when no better owner exists. Repo-owned workflow tool binaries may be flake inputs when they have immutable or versioned URLs and the intended bump path is `nix flake update`.
- No `builtins.fetchurl`, `builtins.fetchTarball`, `builtins.fetchGit`, or `builtins.fetchTree` in tracked Nix files. Use the matching `pkgs.*` fetcher (`pkgs.fetchurl`, `pkgs.fetchzip`, `pkgs.fetchFromGitHub`/`pkgs.fetchgit`, etc.) so the fetch is a fixed-output derivation that can substitute from the cache.
- No fake hash helpers or placeholder hashes in tracked Nix files. Materialize real hashes with the owning updater, lock command, `nix flake update`, or a checked prefetch command before committing.
- No flat top-level `modules` flake output. Use `nixosModules.<name>` (standard schema) for module exports.
- Image target is x86_64-linux only. Host-visible flake package namespaces may include developer systems such as aarch64-darwin, but they should point at the same Linux image derivations rather than changing the image target.
- No stringly `extraConfig` / `extraSettings` options on new modules (RFC 0042). Structured config goes through `pkgs.formats.*` with a freeform submodule; the repo's `configFiles` slot is the canonical path.

## Issues

Keep issue bodies short. State the problem, the context, and the desired outcome. For bug reports, include a `To reproduce` section with the concrete command or steps that exposed the failure. Don't prescribe implementation steps or extra section headers unless explicitly asked.

When creating or editing GitHub issue bodies or comments, pass multiline text through a real multiline input path such as `--body-file -`, a temporary file, or an editor. Do not put escaped `\n` sequences inside a quoted `--body` string; they render literally on GitHub instead of becoming paragraph breaks.

When you hit a real bug, broken assumption, or unidiomatic pattern while working in this repo, file a GitHub issue right then (`gh issue create -R indexable-inc/index ...`). Don't batch and don't wait to be asked. One concrete observation per issue.

Issues should nearly always carry at least one label. `bug` for an observed failure, `enhancement` for a feature ask, `documentation` for doc gaps, `rfc` for design proposals that need discussion before implementation, `help wanted` to signal an open invitation to contributors, and `good first issue` for small well-scoped tasks that don't need deep repo context. Apply labels at filing time so the backlog stays sortable; if an existing issue is unlabeled, label it the next time you touch it. The unlabeled state should be reserved for issues you literally just filed and haven't classified yet.

`ai-capable` marks an issue an AI agent can plausibly finish end-to-end without a human in the loop. Apply it when all of these hold: the desired outcome is concrete (a function, a fix, a doc page) rather than a design call; success is checkable from the issue body alone (tests pass, output matches a sample, eval warning goes away); the scope is bounded enough that a single agent run could finish it; no institutional context is required beyond what is in the repo and linked references. Withhold the label when the issue still names open design questions, asks "should we do X or Y", requires reading a Slack thread / interviewing a teammate, or involves third-party coordination (registry credentials, external service access, vendor decisions). An issue can carry `ai-capable` alongside `good first issue`, `bug`, or `enhancement`; the labels answer different questions ("is the scope small?" vs "is the next step mechanical?").

## Tests

Image and reusable package derivations expose their tests through `passthru.tests.<name>` (RFC 0119). A test that targets one image or one helper attaches to that derivation so `nix build .#<name>.passthru.tests.<test>` works and downstream tooling can iterate. Cross-image eval invariants stay in `tests/default.nix` and remain accessible through `checks.eval`. Tests do not run as part of the default image build.

Use `passthru.tests` for the lengthier or downstream-dependent checks (integration runs, fleet renders, end-to-end image boots). Keep `checkPhase` / `installCheckPhase` for the cheap inline checks that should always run on build.

## Searching

Use `mgrep search -c {natural language}` to search the codebase. Do not use subagents for search.

## Linting

```
nix run .#lint
```

The repo wires `.githooks/pre-commit` to the same lint app via `.envrc`'s `core.hooksPath` override, so `direnv allow` is enough to get the pre-commit run on every `git commit`. CI runs `nix flake check`, which has a single `lint` check that calls the same derivation. There is no separate pre-commit framework to install.
