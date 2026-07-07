{
  lib,
  ix,
  codex,
  rustPlatform,
  makeBinaryWrapper,
  runCommand,
  git,
  nix,
  symlinkJoin,
  formats,
  # Nushell writer for `passthru.updateScript`, pre-bound to the caller's pkgs
  # on the flake path (lib/packages.nix); `null` on the overlay path, which is
  # the signal to omit the fork updater (matches the pins.mkUpdater posture).
  updateScriptWriter ? null,
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
  repoPackages ? {},
  # Upstream openai/codex (codex-src input) with the in-repo patch series
  # (./patches) applied. De-forking replacement for the old
  # `indexable-inc/codex` branch input; the single "route channel notifications
  # into chat" commit is now 0001-*.patch.
  codexSrc ?
    ix.patchedSrc {
      name = "codex";
      src = ix.codexSrc;
      patchDir = ./patches;
    },
  # Rule names dropped from the default house prompt. Only affects the computed
  # `systemPrompt` default below; ignored when `systemPrompt` is passed
  # explicitly.
  omitRules ? [],
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
}: let
  effectiveModelInstructionsFile =
    if modelInstructionsFile != null
    then modelInstructionsFile
    else if systemPrompt != null
    then builtins.toFile "codex-system-prompt.txt" systemPrompt
    else null;

  # The compiled Rust launcher (packages/config-launch): reads IX_LAUNCH_SPEC
  # (a baked JSON file describing the target binary, config path, forced flags,
  # and soft defaults), performs per-key TOML presence detection against the
  # user's config.toml, then exec's the target preserving argv0.
  launcher = ix.rustWorkspace.units.binaries."config-launch";
  entriesOf = flat:
    lib.mapAttrsToList (key: v: {
      inherit key;
      value = ix.toml.scalar v;
    })
    flat;

  # Gates fold in the native tools each baked MCP server supersedes: with the
  # `index` kernel present the codex shell is force-disabled, and the overlay
  # build (no kernel baked) keeps its shell rather than losing every tool.
  sharedPermissions = import (ix.paths.packagesRoot + "/agent/policy/permissions.nix") {
    inherit lib;
    indexKernelBaked = mcpServers ? index;
    exaSearchBaked = mcpServers ? exa;
  };
  effectiveForcedSettings =
    forcedSettings
    // sharedPermissions.codex.forcedSettings
    // {
      features =
        (forcedSettings.features or {}) // (sharedPermissions.codex.forcedSettings.features or {});
    };
  specValue = {
    target = lib.getExe codexWithNotifications;
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
  spec = (formats.json {}).generate "codex-launch-spec.json" specValue;

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
  hooksJson = (formats.json {}).generate "codex-hooks.json" {
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
  codexWithNotifications = codex.overrideAttrs (previousAttrs: {
    version = "0.0.0";
    src = codexSrc;
    # `unpackPhase` names the unpacked dir after the src store path. The old
    # fork input unpacked to `source/`; the patched-src derivation unpacks to
    # its own name (`codex-patched`), so derive the sourceRoot from the src's
    # name rather than hardcoding `source/`.
    sourceRoot = "${codexSrc.name}/codex-rs";
    # No cargoHash: an inlined vendor FOD hash goes stale on every codex-src
    # bump the flake-update bot makes (index #2233 broke every ix deploy this
    # way). importCargoLock needs no aggregate hash: crates.io checksums come
    # from Cargo.lock itself and git deps are fetched by their locked revs.
    # The lockfile is read from the RAW codex-src input (an eval-time store
    # path, so no IFD), not the patched src: no patch touches Cargo.lock
    # today, and if one ever does, cargoSetupPostPatchHook's lock-consistency
    # check fails the build loudly rather than vendoring the wrong set.
    cargoDeps = rustPlatform.importCargoLock {
      lockFile = ix.codexSrc + "/codex-rs/Cargo.lock";
      allowBuiltinFetchGit = true;
    };
    postPatch = ''
      # shell
      # importCargoLock vendors one top-level dir per crate (name-version),
      # unlike fetchCargoVendor's extra nesting level. Version-anchor the glob
      # so the sibling crate webrtc-sys-build-* can never match if a future
      # rust-sdks rev gives it a build.rs (that would break --replace-fail on
      # the next codex-src bump, the breakage class this file just eliminated).
      substituteInPlace $cargoDepsCopy/webrtc-sys-[0-9]*/build.rs \
        --replace-fail "cargo:rustc-link-lib=static=webrtc" "cargo:rustc-link-lib=dylib=webrtc"
      substituteInPlace Cargo.toml \
        --replace-fail 'lto = "thin"' "" \
        --replace-fail 'codegen-units = 4' ""
    '';
    meta =
      previousAttrs.meta
      // {
        homepage = "https://github.com/openai/codex";
        changelog = "https://github.com/openai/codex/commits/main";
      };
  });
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
    name = "codex-${codexWithNotifications.version}";
    paths = [codexWithNotifications];
    # symlinkJoin links the whole codex output (libexec, completions, ...); we only
    # replace the entrypoint with our wrapper so the baked defaults ride every
    # invocation while everything else stays pristine.
    nativeBuildInputs = [makeBinaryWrapper];
    postBuild = ''
      # shell
      rm -f $out/bin/${binName}
      makeBinaryWrapper ${launcher}/bin/config-launch $out/bin/${binName} \
        --inherit-argv0 \
        --set IX_LAUNCH_SPEC ${spec}
    '';
    # The codex hooks.json rendered from the shared declaration list, for a
    # consumer to deliver to `~/.codex/hooks.json` (see the `hooksJson` comment).
    passthru =
      {
        inherit hooksJson spec specValue;
        modelInstructionsFile = effectiveModelInstructionsFile;
        permissions = sharedPermissions.codex;
      }
      # Fork updater (flake path only): bump codex-src and regenerate the patch
      # series, so codex joins the registry-discovered `.#update` DAG. Omitted
      # when the writer or rebase-patches sibling is out of scope (overlay path).
      // lib.optionalAttrs (updateScriptWriter != null && repoPackages ? rebase-patches) {
        updateScript =
          ix.mkForkUpdater {
            writeNushellApplication = updateScriptWriter;
            inherit nix;
            rebasePatches = repoPackages.rebase-patches;
          } {
            name = "codex";
            input = "codex-src";
          };
      };
    meta =
      codexWithNotifications.meta
      // {
        description = "${codexWithNotifications.meta.description or "OpenAI Codex CLI"} (index wrapper with baked defaults)";
        mainProgram = binName;
      };
  }
