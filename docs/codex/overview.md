# codex

`packages/codex` repackages the OpenAI Codex CLI (the nixpkgs `codex` package)
with baked-in `-c` config defaults. The binary is unchanged; the wrapper injects
config on every invocation through the shared `config-launch` launcher
(`packages/config-launch`), the same mechanism
[claude-code](../claude-code/overview.md) uses. The base `codex` is a nixpkgs
dependency, so there is no source pin or updater here; bumping codex is a
nixpkgs/flake-lock bump.

## Why an additive flake output, not an overlay

`package.nix` sets `flake = true` but deliberately NOT `overlay`
(`packages/codex/package.nix:7-13`): `pkgs.codex` must stay the plain nixpkgs
CLI because symphony's room-server wrapper pins `pkgs.codex` as the binary it
spawns over JSON-RPC. This wrapper is an additive output
(`nix run .#codex`, `index.packages.<sys>.codex`) that bakes defaults on top of
the same base without changing what the overlay hands other code.

## Baked defaults

The wrapper builds a launch spec (`packages/codex/default.nix:74-81`) the
launcher reads: it targets `lib.getExe codex`, sets `config_dir_env = CODEX_HOME`
/ `config_dir_default = ~/.codex` / `config_file = config.toml`, and carries two
kinds of `-c key=value` overrides.

### Forced settings (`forcedSettings`, `default.nix:24-26`)

Applied on EVERY invocation (`-c` is codex's highest-precedence layer, above
`~/.codex/config.toml`), so reserved for wrapper invariants the user must not
silently lose. The only one baked: `check_for_update_on_startup = false`, since
the store binary is read-only and the wrapper owns the version pin, so the check
only ever costs a network round-trip it can never act on. Anything
security-shaped (sandbox mode, approval policy) is left to the user's config.

### Soft defaults (`settings`, `default.nix:41-47`)

Injected ONLY when the user's `config.toml` does not already set that exact
dotted-key path; detection is per-leaf via exact TOML path lookup in the
compiled launcher (not substring grep), and a user's own later `-c` still wins.
Defaults bump multi-agent fan-out well above stock:

- `features.multi_agent_v2.enabled = true` and
  `features.multi_agent_v2.max_concurrent_threads_per_session = 16` (stock is 4:
  root + 3 subagents; 16 => root + 15 concurrent subagents). The cap lives under
  the v2 feature because v2 rejects `agents.max_threads`.
- `agents.max_depth = 3` (parent -> child -> grandchild -> great-grandchild),
  still read under v2.

### Baked MCP server (`default.nix:62-73`, `80`)

The `index` MCP server is added as soft `-c mcp_servers.index.*` defaults from
the same `ix.mcp` registry the claude-code wrapper renders, so the kernel is
declared once for both tools. Only stdio servers are baked
(`mcpStdioServers` filters `transport == "stdio"`): codex's streamable-HTTP MCP
support is gated behind version-specific keys, so the keyless `exa` HTTP server
stays claude-only. The `index` server is present only when the `mcp` sibling is
in scope (the flake package set, not the overlay; `repoPackages`,
`default.nix:10-15`, `71`).

### Remote-SSH reach (`default.nix:83-92`)

These baked defaults also reach the Codex GUI app's remote-SSH sessions: the
desktop app bootstraps the host through the remote user's login shell and runs
`codex app-server` from the remote PATH, so whenever this wrapper is the first
`codex` on the remote login-shell PATH it intercepts that launch and injects the
same `-c` flags. Caveats noted inline: the wrapper must win the login shell PATH
(`$SHELL -lc`, which skips `~/.bashrc`/`~/.zshrc`), and a stale already-running
`app-server` is reused without re-injecting (kill it once after a bump).

## Build and wiring

- The wrapper is a `symlinkJoin` over `codex` whose only change is replacing the
  `$out/bin/${binName}` entrypoint with a `makeBinaryWrapper` over the launcher
  (`--inherit-argv0`, `--set IX_LAUNCH_SPEC`), so everything else (libexec,
  completions) stays pristine (`default.nix:93-105`). `binName` defaults to
  `codex`.
- Flake output: `nix run .#codex` / `nix build .#codex`. `package.nix` sets
  `packageSet = true`, `flake = true` (`packages/codex/package.nix`). The
  `packageSet` here is the index package set, not a nixpkgs overlay injection.
- The overlay eval context provides no `repoPackages`, so a `{ }` build bakes no
  MCP server (the same fallback claude-code uses); the flake package set is
  where the `index` server is wired.
- No source pin/manifest/updateScript: the base `codex` and its version come
  from nixpkgs.
