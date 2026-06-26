# claude-code

`packages/agent/claude-code` repackages
[Claude Code](https://www.anthropic.com/claude-code), Anthropic's agentic coding
CLI, as a prebuilt-binary install with a thick layer of baked-in fleet defaults.
The upstream artifact is a Bun single-file executable pinned per platform; this
package wraps it so every session starts with the repo's flags, env, settings,
MCP servers, system prompt, and hooks already applied, while keeping the store
binary itself read-only and pristine. It is the richest wrapper in this domain.

The injection is done through the shared `config-launch` launcher
(`packages/config-launch`), the same mechanism [codex](../codex/overview.md)
uses: the wrapper bakes a launch-spec JSON and points `config-launch` at it via
a `makeBinaryWrapper` `--set IX_LAUNCH_SPEC`
(`packages/agent/claude-code/default.nix:451-459`).

## Version pin and provenance

`manifest.json` holds `version` and per-platform `{ slug, hash }`, read with
`lib.importJSON` and never hand-edited (`packages/agent/claude-code/default.nix:128-129`;
`manifest.json` currently pins `2.1.170` for four platforms). The binary is
`fetchurl`ed from the Anthropic-branded CDN with the GCS bucket as a hash-pinned
mirror (`packages/agent/claude-code/default.nix:402-408`).

- Bump: `nix run .#claude-code.updateScript -- [version]`. The updater
  (`update.nix`) refetches Anthropic's per-version manifest, converts its hex
  checksums to SRI, and rewrites `manifest.json`
  (`packages/agent/claude-code/update.nix:33-67`).
- Fails closed on provenance: the updater verifies the manifest's detached GPG
  signature against the pinned release key (`release-signing-key.asc`,
  fingerprint `31DD DE24 ... 1A7E CACE`) in an isolated `GNUPGHOME` and aborts
  if `gpg --verify` is non-zero, so a spoofed manifest cannot inject hashes for
  attacker-controlled binaries (`packages/agent/claude-code/update.nix:1-8`, `43-52`).
- Pin is by raw version, not the npm `latest` tag, because Anthropic ships to
  the `next` prerelease tag days before promoting to `latest`
  (`packages/agent/claude-code/default.nix:121-127`).

## Baked defaults

All flag/env/PATH/settings injection is declared as data in `launchSpec`
(`packages/agent/claude-code/default.nix:375-391`) and applied by `config-launch`.

### Forced env (always set)

Because the store output is read-only, the bundled self-updater could never
write, so the wrapper turns it off hard (`default.nix:375-381`):
`DISABLE_AUTOUPDATER=1`, `DISABLE_INSTALLATION_CHECKS=1`, and
`USE_BUILTIN_RIPGREP=0` (search uses the Nix `ripgrep` on PATH so the wrapper
owns the version pin). The wrapper also exports `IX_CLAUDE_SKILLS_DIR` and,
when the full flake package set is in scope, `IX_CLAUDE_AGENTS_DIR`: these are
prebuilt store paths for the repo SessionStart materializer, so startup copies
agent content from Nix outputs instead of invoking `nix build` interactively.

### Soft env defaults (set only when unset)

`env_defaults` are applied only if the user has not set them
(`default.nix:147-152`, `382`): `CLAUDE_CODE_DISABLE_1M_CONTEXT=1` keeps every
session on the standard 200K window instead of the silently auto-upgraded 1M
window (uncached, slower per turn). Re-enable per machine with
`export CLAUDE_CODE_DISABLE_1M_CONTEXT=`.

### Prepended flags (`wrapperFlags`, `default.nix:353-361`)

Flags ride BEFORE the user argv (so root options parse before subcommand
dispatch, e.g. `claude --settings=F mcp list`), and every option-argument uses
the `=` form (one self-contained token, so a variadic flag cannot swallow a
positional). Both rules are learned from real breakage; see the long comment at
`default.nix:315-352`.

- `--debug`: writes operational telemetry to `~/.claude/debug/` (pruned on the
  `cleanupPeriodDays` sweep). It is an optional-value flag, so it cannot take
  `=`; it is safe only because `--thinking-display` follows it.
- `--thinking-display=summarized`: forces visible reasoning. The API default
  flipped to "omitted" on Opus 4.7/4.8, hiding thinking in the UI and
  transcript; this hidden flag is the only lever that restores it (verified on
  2.1.159).
- `--dangerously-skip-permissions` (default on): start every session with the
  permission layer skipped (the fleet runs trusted configs in disposable
  sandboxes). Gated on `dangerouslySkipPermissions` (below). Mind the upstream
  uid-0 guard: the CLI refuses this flag for an unsandboxed root user, so root
  consumers carry their own `IS_SANDBOX=1` or turn the flag off.
- `--system-prompt-file=<store path>`: the house system prompt (below),
  baked by path so content needs no shell quoting. It REPLACES the stock prompt
  rather than appending to it.
- `--mcp-config=<file>`: the baked MCP server set (below).

### Conditional `--settings` (injected only when the caller passes none)

The CLI is first-wins between two `--settings` flags (they do not merge), so the
wrapper injects its defaults file only `unless_present` a caller `--settings`
(`default.nix:385-390`, `conditional_flags`). The file is a deep-merge of caller
`extraSettings` UNDER the computed defaults so package-owned keys always win
(`default.nix:226-293`):

- `cleanupPeriodDays = 365`: keep transcripts and `--debug` logs ~1yr.
- `skipDangerousModePermissionPrompt = true` (when
  `dangerouslySkipPermissions`): pre-accept the one-time dangerous-mode warning
  the flag alone does not suppress.
- `permissions.ask`: `gh pr merge --admin` / `--force` pause for confirmation
  so the local-build gate in the system prompt is applied (postmortem
  ENG-2391); ask rules are not enforced under the baked skip-permissions, so
  this is the practical gate for consumers who turn the flag off.
- `permissions.deny` `WebSearch` / `WebFetch`: one web surface, not two; use
  Exa MCP for live web research. Deny rules are enforced in every
  permission mode.
- `hooks` (below).

### MCP servers (`--mcp-config`, `default.nix:90-94`, `295-297`)

Rendered from the shared `ix.mcp` registry (`lib/util/mcp.nix`) so `index` is
declared once for both this wrapper and codex. CLI `--mcp-config` layers merge,
so a user's `--mcp-config` and a project `.mcp.json` still load alongside.
Defaults to the default pair, additions only:

- `index`: the ix notebook kernel (`ix-mcp serve`, `packages/mcp`) over stdio, present
  only when the `mcp` sibling is in scope (the flake package set, not the
  overlay; see `repoPackages`, `default.nix:59-69`).
- `exa`: Exa's hosted web-search server over streamable HTTP at
  `https://mcp.exa.ai/mcp` (keyless, rate-limited).

### System prompt (`system-prompt.nix`)

`systemPrompt` is baked as the session's system prompt, REPLACING the stock one
rather than appending to it (`default.nix:95-113`). The text is the shokunin craft ethos plus
fleet engineering rules: pre-v1 no-backward-compatibility, one-concept-one-
implementation, always work in a git worktree, spawn background subagents for
independent work, do work through the index Python kernel and `search` priors,
gate admin/force merges on a fresh local build, never use em dashes, and more
(`system-prompt.nix:7-37`). Set to `null` to ship the stock prompt alone.

### Hooks (`packages/agent/policy/hook-runner.nix`, `default.nix:209-285`)

Lifecycle hooks, all subcommands of one compiled binary (`packages/agent/claude-hooks`)
wrapped with their tool paths and the baked primary-checkout default; each fails
open and silent
(`packages/agent/policy/hook-runner.nix:1-20`):

- `SessionStart` -> `session-digest`: cats the pre-rendered fleet context digest
  (`~/.cache/ix/context-digest.md`), capped ~6000 chars. Kill switch
  `CLAUDE_CODE_DISABLE_CONTEXT_DIGEST=1`.
- `PreToolUse` (Edit|MultiEdit|Write|NotebookEdit) -> `worktree-guard`: denies a
  file-edit whose TARGET path resolves into a protected primary checkout
  (judging the target, not the session cwd, which closes the
  project-hook bypass, ENG-2692). Protected globs default to `/home/*/index` and
  `/home/*/ix` (`default.nix:54-57`), overridable per machine via
  `CLAUDE_CODE_PRIMARY_CHECKOUTS`; `[ ]` disables it. Kill switch
  `CLAUDE_CODE_DISABLE_WORKTREE_GUARD=1`.
- `UserPromptSubmit` -> `prompt-priors` (only when the `search` sibling is in
  scope): score-gated ambient priors from the corpus store, capped ~1200
  tokens. Kill switch `CLAUDE_CODE_DISABLE_PROMPT_PRIORS=1`.

### Prepended PATH (`pathPrepend`, `default.nix:299-313`)

`procps` (process checks), the pinned `ripgrep`, the `minecraft-sound` chime,
and on Linux the sandbox helpers `bubblewrap` and `socat`.

## Overrides

`default.nix` exposes these args: `binName` (default `claude`),
`dangerouslySkipPermissions`, `extraSettings`, `primaryCheckouts`, `mcpServers`,
`systemPrompt`. Example: `claude-code.override { dangerouslySkipPermissions = false; }`.

## Build and wiring

- Install (`default.nix:435-462`): the real binary is installed to
  `$out/libexec/Claude Code` (off PATH, named for the product so 1Password's
  "CLI access requested" prompt reads "Claude Code"); the launch spec's
  `@helper@` placeholder is substituted with its real path; `$out/bin/${binName}`
  is a `makeBinaryWrapper` over `config-launch` with `--inherit-argv0` and
  `--set IX_LAUNCH_SPEC`. `dontStrip = true` because stripping corrupts Bun's
  appended trailer (`default.nix:425-427`); `autoPatchelfHook` runs on ELF
  hosts.
- Install checks (`install-check.nix`): an offline argv regression net driven
  through the real launcher against a stub target (flags prepend, `=` form,
  `--settings` defers to the caller) plus behavioral nets for all three hooks
  (`default.nix:464-480`).
- Flake output: `nix run .#claude-code` / `nix build .#claude-code`, plus
  `pkgs.claude-code` (overlay). `package.nix` sets `packageSet`, `flake`,
  `overlay`, `updateScript` all `true` (`packages/agent/claude-code/package.nix`).
  Note: the overlay build gets `repoPackages = { }`, so it drops sibling-
  dependent defaults (the `index` MCP server, the search-gated prompt-priors
  hook) and omits `passthru.updateScript`; the full-featured build is the flake
  package set.
- Platforms: aarch64/x86_64 darwin and linux (the four `manifest.json` keys).
  `meta.license` is omitted so the no-`allowUnfree` flake set can still build it
  (`default.nix:489-492`).
