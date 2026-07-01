{
  lib,
  ix,
  codex,
  makeBinaryWrapper,
  runCommand,
  git,
  symlinkJoin,
  formats,
  binName ? "codex",

  # Shell globs the (claude-only) worktree-guard protects, threaded into the
  # shared hook module so both wrappers feed it the same inputs. Unused in the
  # codex render (worktree-guard is claude-only), kept only for parity.
  primaryCheckouts ? [
    "/home/*/index"
    "/home/*/ix"
  ],

  # Andrew-only local startup context: cached notes and ~/Projects inventory.
  # Disabled for the shared wrapper because those hooks print workstation-local
  # context that is not meaningful for other users.
  personalStartupContext ? false,

  # Sibling repo packages from the flake package set (threaded by
  # lib/packages.nix), used to locate the `ix-mcp` entrypoint for the baked
  # `index` MCP server. `{ }` in the overlay package set, where the `mcp`
  # sibling is out of scope, so the wrapper bakes no MCP server there (the same
  # fallback the claude-code wrapper uses).
  repoPackages ? { },

  # Rule names dropped from the default house prompt. Only affects the computed
  # `systemPrompt` default below; ignored when `systemPrompt` is passed
  # explicitly.
  omitRules ? [ ],

  # Forced config: codex `-c key=value` overrides applied on EVERY invocation.
  # `-c` is codex's highest-precedence layer (above ~/.codex/config.toml), so use
  # this ONLY for wrapper INVARIANTS the user must not silently lose. The one we
  # bake: turn off the startup update check, since the store binary is read-only
  # and the wrapper owns the version pin, so the check only ever costs a network
  # round-trip it can never act on. Shared house policy also lands here when it
  # must outrank mutable user config, such as disabling superseded shell tools.
  # Broader sandbox and approval posture stays in the user's config or Codex's
  # managed requirements layer.
  forcedSettings ? {
    check_for_update_on_startup = false;
  },

  # Soft defaults: codex `-c key=value` flags injected ONLY when the user's
  # config.toml does not already configure that exact dotted-key path, so an
  # explicit user value always wins. Detection is per-leaf (exact TOML path
  # lookup via the compiled Rust launcher, not substring grep): a config that
  # sets `features.multi_agent_v2.enabled` keeps only that key out of the
  # wrapper defaults, while sibling keys (like max_concurrent_threads_per_session)
  # are still injected if unset. A user's own later `-c` still wins over both.
  #
  # Default: a much higher multi-agent fan-out than stock. Run the v2 runtime
  # (stock default 4 = root + 3 subagents); 16 => root + 15 concurrent subagents.
  # v2 REJECTS `agents.max_threads` ("cannot be set when multi_agent_v2 is
  # enabled"), so the cap lives under the v2 feature; only `agents.max_depth` is
  # still read under v2 (3 => parent -> child -> grandchild -> great-grandchild).
  settings ? {
    features.multi_agent_v2 = {
      enabled = true;
      max_concurrent_threads_per_session = 16;
    };
    agents.max_depth = 3;
  },

  # MCP servers rendered as soft Codex defaults. A user's own
  # `[mcp_servers.<name>]` config wins per-key through config-launch.
  mcpServers ?
    (import (ix.paths.packagesRoot + "/agent/common.nix") {
      inherit lib ix repoPackages;
      promptOmitRules = omitRules;
    }).defaultServers,

  # The house model/base instructions Codex should run with. This becomes a
  # store-backed `model_instructions_file` soft default. Null bakes no default.
  systemPrompt ?
    (import (ix.paths.packagesRoot + "/agent/common.nix") {
      inherit lib ix repoPackages;
      promptOmitRules = omitRules;
    }).systemPromptFor
      "codex",

  # Existing prompt file to use instead of materializing `systemPrompt`.
  # Overrides `systemPrompt` when non-null.
  modelInstructionsFile ? null,
}:
let
  effectiveModelInstructionsFile =
    if modelInstructionsFile != null then
      modelInstructionsFile
    else if systemPrompt != null then
      builtins.toFile "codex-system-prompt.txt" systemPrompt
    else
      null;

  # The compiled Rust launcher (packages/config-launch): reads IX_LAUNCH_SPEC
  # (a baked JSON file describing the target binary, config path, forced flags,
  # and soft defaults), performs per-key TOML presence detection against the
  # user's config.toml, then exec's the target preserving argv0.
  launcher = ix.rustWorkspace.units.binaries."config-launch";
  entriesOf =
    flat:
    lib.mapAttrsToList (key: v: {
      inherit key;
      value = ix.toml.scalar v;
    }) flat;

  sharedPermissions = import (ix.paths.packagesRoot + "/agent/policy/permissions.nix") {
    inherit lib mcpServers;
  };
  effectiveForcedSettings =
    forcedSettings
    // sharedPermissions.codex.forcedSettings
    // {
      features =
        (forcedSettings.features or { }) // (sharedPermissions.codex.forcedSettings.features or { });
    };
  specValue = {
    target = lib.getExe codex;
    config_dir_env = "CODEX_HOME";
    config_dir_default = "~/.codex";
    config_file = "config.toml";
    forced = entriesOf (ix.attrs.flattenToDotted effectiveForcedSettings);
    soft =
      entriesOf (
        ix.attrs.flattenToDotted (
          lib.optionalAttrs (effectiveModelInstructionsFile != null) {
            model_instructions_file = toString effectiveModelInstructionsFile;
          }
          // settings
        )
      )
      ++ ix.mcp.toCodexEntries mcpServers;
  };
  spec = (formats.json { }).generate "codex-launch-spec.json" specValue;

  # Codex reads hooks from config, not from launch flags, so expose the rendered
  # shared hook policy for home-manager or managed requirements consumers.
  hookRunner = import (ix.paths.packagesRoot + "/agent/policy/hook-runner.nix") {
    inherit
      lib
      runCommand
      makeBinaryWrapper
      ix
      git
      primaryCheckouts
      repoPackages
      ;
  };
  hooksJson = (formats.json { }).generate "codex-hooks.json" {
    hooks =
      (import (ix.paths.packagesRoot + "/agent/policy/hooks.nix") {
        inherit
          lib
          hookRunner
          primaryCheckouts
          personalStartupContext
          ;
      }).codex;
  };

in
# These baked defaults also reach the Codex GUI app's remote-SSH sessions, not
# just terminal use. The desktop app does NOT ship its own binary to the remote
# (unlike VS Code Remote SSH): it bootstraps the host through the remote user's
# login shell and runs `codex app-server` from the remote PATH (then connects via
# `codex app-server proxy`). So whenever THIS wrapper is the `codex` first on the
# remote's login-shell PATH, it intercepts that `app-server` launch and injects
# the same `-c` flags, and every GUI/phone session against that host inherits the
# defaults. Caveats: the wrapper must win the remote *login* shell PATH (the probe
# uses `$SHELL -lc`, which skips ~/.bashrc/~/.zshrc), and a stale already-running
# `codex app-server` is reused without re-injecting, so kill it once after a bump.
symlinkJoin {
  name = "codex-${codex.version}";
  paths = [ codex ];
  # symlinkJoin links the whole codex output (libexec, completions, ...); we only
  # replace the entrypoint with our wrapper so the baked defaults ride every
  # invocation while everything else stays pristine.
  nativeBuildInputs = [ makeBinaryWrapper ];
  postBuild = ''
    # shell
    rm -f $out/bin/${binName}
    makeBinaryWrapper ${launcher}/bin/config-launch $out/bin/${binName} \
      --inherit-argv0 \
      --set IX_LAUNCH_SPEC ${spec}
  '';
  # The codex hooks.json rendered from the shared declaration list, for a
  # consumer to deliver to `~/.codex/hooks.json` (see the `hooksJson` comment).
  passthru = {
    inherit hooksJson spec specValue;
    modelInstructionsFile = effectiveModelInstructionsFile;
    permissions = sharedPermissions.codex;
  };
  meta = codex.meta // {
    description = "${codex.meta.description or "OpenAI Codex CLI"} (index wrapper with baked defaults)";
    mainProgram = binName;
  };
}
