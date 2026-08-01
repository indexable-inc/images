---
name: linting
description: "Which lint command each repo has -- index has `nix run .#lint`, ix has `just lint` -- and why an ambient ruff or clippy is not a substitute for either. Use before committing or when a lint check fails."
---

## There is no universal lint command; check the repo first

```sh
git remote get-url origin
```

| origin | lint the whole tree |
|--------|---------------------|
| `indexable-inc/index` | `nix run .#lint` |
| `indexable-inc/ix` | `just lint` |

`ix` vendors index's history under `ix/index/`, so one checkout holds both. The
origin is what decides, not the directory you started in: inside `ix/index/` you
are in an ix checkout, and `just lint` is the command for the whole thing.

Neither command is platform-gated. Both run on darwin and Linux.

Do not reach for the other repo's spelling when yours errors, and do not add a
wrapper so the two match. ix has no `nix run .#lint` on purpose:
`agent-context/sections/40-nix-stack.md` forbids Nix apps that invoke `nix`
internally, and `just lint` is `nix build -L --no-link .#ci-lint-checks` -- the
same derivation CI's lint phase builds, resolved to the running system.

That difference is worth stating this loudly because of what hiding it cost.
This skill used to print `nix run .#lint` as though it were universal. Nine
agents ran it in ix, got `does not provide attribute`, correctly concluded lint
was unavailable to them, and each filed the same ticket (ENG-9808 and eight
duplicates). The command had never existed in ix on any platform, so every one
of those reports diagnosed a darwin gap that was not there.

## Neither repo runs any git hook, so nothing lints for you

There is no `.githooks/` and no `.pre-commit-config.yaml` in either tree, and
nothing is installed into `.git/hooks`. Cloning is the whole setup. index's last
hook went on 2026-07-19 (the `drop-direnv-and-hooks` site update) and ix's three
went in ENG-11624, once every check they ran had a CI gate that was the same or
wider.

So a clean commit and a clean push say nothing about lint. Run the command in the
table above before you push, or read the verdict off CI after you do. Those are
the only two ways to know.

Do not install a hook to get the check back. `pre-commit install` in either repo
refuses with `No .pre-commit-config.yaml file was found`, and writing one is how
this went wrong the first time: ix had a config for its whole life that
`core.hooksPath` made unreachable, so the `no-raw-config-in-nix` rule listed in
it had never run on a single commit. A gate reporting nothing is indistinguishable
from a gate reporting clean.

## Run every gate through nix, never an ambient tool

A pinned tool and the one on your PATH are different tools. The flake fixes
versions and flags; ambient binaries do not, so a check that passes ad-hoc can
still fail the gate and vice versa. In index, ruff is the everyday example: the
flake pins it and passes a fixed `--target-version`, so version-gated rules
(`UP041`, `PERF203`) fire in one and not the other.

Rust is where the gap is widest and fails most quietly. Both repos build Rust
against a jj megamerge fork of clippy carrying eleven restriction lints that
stock clippy has never heard of (`packages/llm-clippy`, `lib/fork-packages.nix`
in index; ix reaches the same fork through its per-unit clippy graphs in
`lib/workspace-cargo-unit.nix`). Stock clippy does not skip an unknown lint; it
raises `error[E0602]: unknown lint` and the crate never compiles, so no lint
pass runs over the code at all.

That failure is dangerous because of its shape: the run produces no findings,
and no findings looks like a pass. On 2026-07-28 an agent ran `cargo clippy`,
grepped the output for findings, found none, and reported the crate clean. The
compile had aborted. Running the forked driver over the same code found two
real violations, both already merged: a struct over the three-bool limit and a
function over the hundred-line limit.

So check the exit code, never the absence of a string in a log. `grep -c error`
returning zero is not a pass when the exit code was 101.

### And an empty run is the good case; the bad one is a full one

On 2026-08-01 the same command produced the opposite shape, which is worse. An
agent ran `cargo clippy -p ix2nix -- -D clippy::doc_markdown` to confirm a CI
clippy failure was fixed. Stock clippy raised `E0602` for both fork lints, as
above, **and then emitted fourteen `doc_markdown` findings anyway**, in
`lib.rs`, `ty.rs`, `map.rs`, `checker.rs` and `schema.rs`.

The crate's CI clippy check was green before that day's change, so the forked
driver flags none of those fourteen. They are the stock driver's findings for a
lint both drivers implement and disagree about.

So the abort does not reliably leave you with nothing. It can leave you with a
plausible, non-empty, entirely irrelevant report, and a report is far more
convincing than silence. An agent reading it would either chase fourteen
phantom defects or, having fixed the real one, conclude from the remaining
thirteen that the fix had failed.

The rule that survives both shapes: a stock clippy run is not evidence about
this repo's clippy, whatever it prints. Only the forked driver or the per-crate
gate answers the question.

In index, put the pinned driver on PATH directly:

```sh
nix build .#llm-clippy --no-link --print-out-paths   # then PATH=<out>/bin:$PATH
```

ix exposes no such output, and `just lint` does not cover Rust there at all.

### On ix, clippy does not gate ix's own crates

Do not reach for `nix build .#ciChecks.<system>.rust-<crate>.clippy` on ix. The
attribute evaluates and builds, so it looks like the gate, and nothing in the
required job set realizes it. An agent who runs it gets a real answer to a
question CI never asks, and an agent who skips it loses nothing.

Unrealized by construction rather than by accident, in
`nix/packages/workspace-rust-ci.nix`:

```nix
clippyChecksByPackage = allBuildWorkspace.clippyByPackage;   # :497, computed

pkgs.runCommand "rust-workspace-build" {
  deps = allMainAndBinaryRoots ++ builtins.attrValues allTestChecksByTarget;
}                                                            # :769, no clippy

checks = { rust-checks-all = build; } // kvmChecks;          # :820
```

`clippyChecksByPackage` is computed and exported in the top-level `inherit`
(:801) and never appears in `build.deps`. So `rust-checks-all`, the required
Rust gate, carries 442 `cargo-unit-nextest-*` derivations and zero clippy runs.
The only clippy in its closure is the driver itself,
`clippy-preview-...-nightly` and its tarball: fetched into the build
environment, never invoked.

**index's crates are covered, and by a mechanism worth copying rather than
reinventing.** ix#9288 folded `index.requiredGateRoots` into ix's required set,
and each `index-rust-<crate>` aggregate pulls that crate's clippy in as a build
dependency. That is why an `ix2nix-clippy` or `roots-clippy` failure can block a
pull request while an ix-owned crate's clippy runs nowhere. The vendored tree
has better Rust coverage than the repo vendoring it.

Practical consequence while that stands: for an ix-owned crate, a local run is
the only clippy anyone will do. That does not make a stock `cargo clippy` into
evidence, for the reasons above, but it does mean the differential is worth
running rather than deferring to a gate that is not there.

## A green `nix run .#lint` does not mean the clone gate is green

The lint app runs the clone detector's GLOBAL gate (whole-tree duplication under
`[budget] global_pct`) and NOT its DIFF gate (duplication over the lines this
branch changed, budget `0%`). The exclusion is deliberate: the diff gate needs a
`.git` directory to resolve the merge base, and the CI lint derivation is handed
a `.git`-less source tree. It is also invisible, because a lint run says nothing
about the gate it did not run.

So `flake-check` can fail on duplication minutes after 15/15 stages passed
locally. Reproduce it before pushing:

```sh
nix run .#clone -- . --diff origin/main --pretty
```

`--pretty` is what makes the result actionable. The JSON names each clone
instance's `fragments` with file and line ranges, which turns "33/793 changed
lines duplicated" into "you copied `render_table` out of `drift.rs`". Without it
you get a percentage and no location.

Both gates are ratchets, so the fix is to delete the duplication rather than to
raise a budget: extract the shared thing and confirm whole-tree duplication went
DOWN. On index#4497 that was a copied markdown-table renderer plus a fifth copy
of an enum-to-string `match`; factoring them into one module took the tree from
0.2300% to 0.2246% and the diff gate to 0/865.

## index only: --json and --fix


Both are flags of index's lint app, which ix has no equivalent of. `just lint`
forwards its arguments to `nix build`, so `just lint --json` is nix's `--json`
and means something else entirely.

`nix run .#lint -- --json` prints one JSON array with a record per check --
`{check, ok, output}`, where `output` carries the stage's diagnostics on
failure -- and still exits nonzero when any check fails. Use it to load results
as a dataframe rather than grepping the log.

`nix run .#lint -- --fix` auto-applies the fixable findings (alejandra, statix
and deadnix over `.nix`, `ruff --fix` over `.py`): it builds the fixer lanes as
derivations and `git apply`s the resulting patch to the worktree. Commit your
work first -- Nix snapshots the committed state in a linked worktree, and
`git apply` refuses on context mismatch instead of clobbering uncommitted
edits. Verdict-only stages (astlog, filenames, dirnames, clone) and unfixable
findings still need hand edits; re-run the lint afterwards.
