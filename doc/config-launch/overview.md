# config-launch

`packages/config-launch` is a spec-driven exec launcher: it reads a JSON spec,
sets environment variables and `PATH`, injects CLI flags (static, and
config-file-gated `--config`), then `exec`s a target binary while preserving
argv0. It is a small Rust workspace crate used to wrap third-party CLIs
(codex, claude-code) with repo defaults.

## Purpose

Several vendored CLIs need to launch with ix-specific environment and flag
defaults, but each defaults differently and should not be patched. config-launch
moves that policy into a declarative JSON spec a Nix wrapper writes, so one tiny
launcher handles every case and `exec`s the real binary (no extra process in the
tree). Because it `exec`s rather than spawns and sets argv0 to the original argv0
(`src/main.rs:121`, `:138`), the target sees itself as if invoked directly.

## Spec (`IX_LAUNCH_SPEC`, `src/main.rs:15`)

The launcher reads the path in `IX_LAUNCH_SPEC` and parses it as the `Spec` JSON
(`load_spec`, `src/main.rs:105`). Every field beyond `target` is optional, so each
consumer uses only the layers it needs (`src/main.rs:15-18`):

- `target` (`:22`): the real binary to `exec`.
- **Generic launcher layers**:
  - `env` (`:40`): a name-to-value object of variables set unconditionally.
  - `env_defaults` (`:44`): same shape, set only when not already present in the
    caller's environment (the `export NAME="${NAME-default}"` idiom).
  - `path_prepend` (`:47`): directories prepended ahead of the caller's `PATH`.
  - `flags` (`:50`): flags prepended before the user argv, unconditionally.
- **codex `--config k=v` layer**:
  - `forced` (`:33`): `--config key=value` injected always.
  - `soft` (`:35`): `--config key=value` injected only when the dotted key is
    absent from the target's config file.
  - `config_dir_env` / `config_dir_default` / `config_file` (`:27-31`): locate
    that config file (env var, else a `~`-expanded default dir, joined with the
    file name; `config_path`, `:60`).

The launcher deliberately carries no settings-file layer: claude-code's
computed settings render is materialized into the writable user
`settings.json` instead of riding argv, where a CLI `--settings` flag would
outrank the user's own settings files (#3180).

## Order of operations (`main`, `src/main.rs:111`)

1. Load the spec; a missing/unreadable/invalid spec exits 78 (`EX_CONFIG`,
   `:114-117`).
2. Split argv into argv0 and the user args (`:120-122`).
3. Read the config file only when there are `soft` keys whose presence it gates
   (claude-code sets none, so it never needs a config dir; `:126-132`).
4. Build the prepended args: `flags`, then the `--config` layer
   (`build_config_flags`, `:80`).
5. Build the command: `arg0(argv0)`, apply `env` then `env_defaults` (only when
   unset), prepend `path_prepend` to `PATH` (`build_path`, `:96`), then the
   prepended flags, then the user args (`:137-152`).
6. `exec` the target; a failed `exec` prints an error and exits 127 (`:154-156`).

## Soft gating semantics

- **`is_set`** (`src/main.rs:67`): a `soft` key is withheld only when its exact
  dotted path is present in the parsed TOML config; a sibling leaf under the same
  table is still injected (tested at `:271-316`). Partial paths count as present
  (`features.multi_agent_v2` is set when `features.multi_agent_v2.enabled` is).
- **`forced`** always wins, even over a user config that sets the key
  (`:222-243`).

## Build and packaging

`default.nix` selects the binary via `ix.cargoUnit.selectBinaryWithTests` (MIT).
It is `inRustWorkspace`, `flake = true`, `packageSet = true`. Flake output /
main program: `config-launch`. Deps: `serde`/`serde_json` (spec) and `toml`
(target config). Unix-only (`std::os::unix::process::CommandExt` for `arg0`/
`exec`, `src/main.rs:3`). Tests in `src/main.rs` and `tests/cli.rs`.
