# config-launch

`packages/config-launch` is a spec-driven exec launcher: it reads a JSON spec,
sets environment variables and `PATH`, injects CLI flags (static,
argv-conditional, and config-file-gated `--config`), then `exec`s a target binary
while preserving argv0. It is a small Rust workspace crate used to wrap
third-party CLIs (codex, claude-code) with repo defaults.

## Purpose

Several vendored CLIs need to launch with ix-specific environment and flag
defaults, but each defaults differently and should not be patched. config-launch
moves that policy into a declarative JSON spec a Nix wrapper writes, so one tiny
launcher handles every case and `exec`s the real binary (no extra process in the
tree). Because it `exec`s rather than spawns and sets argv0 to the original argv0
(`src/main.rs:163`, `:180`), the target sees itself as if invoked directly.

## Spec (`IX_LAUNCH_SPEC`, `src/main.rs:28`)

The launcher reads the path in `IX_LAUNCH_SPEC` and parses it as the `Spec` JSON
(`load_spec`, `src/main.rs:147`). Every field beyond `target` is optional, so each
consumer uses only the layers it needs (`src/main.rs:23-26`):

- `target` (`:30`): the real binary to `exec`.
- **Generic launcher layers**:
  - `env` (`:48`): variables set unconditionally.
  - `env_defaults` (`:52`): set only when not already present in the caller's
    environment (the `export NAME="${NAME-default}"` idiom).
  - `path_prepend` (`:55`): directories prepended ahead of the caller's `PATH`.
  - `flags` (`:58`): flags prepended before the user argv, unconditionally.
  - `conditional_flags` (`:61`): flag blocks (`ConditionalFlags`, `:18`) prepended
    only when the user passed no equivalent option.
- **codex `--config k=v` layer**:
  - `forced` (`:41`): `--config key=value` injected always.
  - `soft` (`:43`): `--config key=value` injected only when the dotted key is
    absent from the target's config file.
  - `config_dir_env` / `config_dir_default` / `config_file` (`:34-39`): locate
    that config file (env var, else a `~`-expanded default dir, joined with the
    file name; `config_path`, `:71`).

## Order of operations (`main`, `src/main.rs:153`)

1. Load the spec; a missing/unreadable/invalid spec exits 78 (`EX_CONFIG`,
   `:156-159`).
2. Split argv into argv0 and the user args (`:162-164`).
3. Read the config file only when there are `soft` keys whose presence it gates
   (claude-code sets none, so it never needs a config dir; `:168-174`).
4. Build the prepended args: `flags` plus each conditional block the user did not
   override (`build_arg_flags`, `:127`), then the `--config` layer
   (`build_config_flags`, `:91`).
5. Build the command: `arg0(argv0)`, apply `env` then `env_defaults` (only when
   unset), prepend `path_prepend` to `PATH` (`build_path`, `:138`), then the
   prepended flags, then the user args (`:179-194`).
6. `exec` the target; a failed `exec` prints an error and exits 127 (`:196-198`).

## Conditional and soft gating semantics

- **`arg_present`** (`src/main.rs:108`): a user arg counts as overriding a
  conditional block when it equals one of the block's `unless_present` names or
  starts with `"<name>="`. Scanning stops at the first `--`, so a value after `--`
  is treated as a positional, not the option (`:111`, tested at `:459`).
- **`is_set`** (`src/main.rs:78`): a `soft` key is withheld only when its exact
  dotted path is present in the parsed TOML config; a sibling leaf under the same
  table is still injected (tested at `:330-363`). Partial paths count as present
  (`features.multi_agent_v2` is set when `features.multi_agent_v2.enabled` is).
- **`forced`** always wins, even over a user config that sets the key
  (`:280-299`).

## Build and packaging

`default.nix` selects the binary via `ix.cargoUnit.selectBinaryWithTests` (MIT).
It is `inRustWorkspace`, `flake = true`, `packageSet = true`. Flake output /
main program: `config-launch`. Deps: `serde`/`serde_json` (spec) and `toml`
(target config). Unix-only (`std::os::unix::process::CommandExt` for `arg0`/
`exec`, `src/main.rs:2`). Tests in `src/main.rs` and `tests/cli.rs`.
