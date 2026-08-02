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

`index` is a submodule of `ix`, so one checkout holds both. The origin is what
decides, not the directory you started in: inside `ix/index/` you are in index
and `nix run .#lint` is correct.

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

## No commit hook runs the full lint in either repo

Do not treat a green commit as a green lint. What actually runs:

- **index**: nothing. There is no `.githooks/`, no `.pre-commit-config.yaml`,
  and no installed hook -- the last one was removed on 2026-07-19 (the
  `drop-direnv-and-hooks` site update). Cloning is the whole setup, and running
  the lint is on you.
- **ix**: `.githooks/pre-commit`, installed by entering the devshell, which
  points `core.hooksPath` at `.githooks/`. It runs astlog over the staged
  `.nix` files -- 1 of the 15 stages in the bundle -- and prints the stages it
  skipped on every commit, including the ones it passes. Take that footer
  literally.

ix also carries a `.pre-commit-config.yaml`, and it is not the installed hook.
`core.hooksPath` and the pre-commit framework are mutually exclusive: with the
path set, `pre-commit install` refuses outright; unset it and the framework's
`.git/hooks/pre-commit` runs *instead of* `.githooks/pre-commit`, silently
dropping the submodule-gitlink/flake.lock desync refusal. Never run
`pre-commit install` in ix.

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

In index, put the pinned driver on PATH directly:

```sh
nix build .#llm-clippy --no-link --print-out-paths   # then PATH=<out>/bin:$PATH
```

ix exposes no such output -- `just lint` does not cover Rust there at all
(clippy runs in the `rust` CI phase, per cargo-unit crate). Reach it through
the unit build rather than a bare `cargo clippy`.

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
