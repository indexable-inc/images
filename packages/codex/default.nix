{
  lib,
  ix,
  codex,
  makeBinaryWrapper,
  symlinkJoin,
  formats,
  binName ? "codex",

  # Forced config: codex `-c key=value` overrides applied on EVERY invocation.
  # `-c` is codex's highest-precedence layer (above ~/.codex/config.toml), so use
  # this ONLY for wrapper INVARIANTS the user must not silently lose. The one we
  # bake: turn off the startup update check, since the store binary is read-only
  # and the wrapper owns the version pin, so the check only ever costs a network
  # round-trip it can never act on. Anything security-shaped (sandbox mode,
  # approval policy) is left to the user's config and codex's requirements layer.
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
}:
let
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
  spec = (formats.json { }).generate "codex-launch-spec.json" {
    target = lib.getExe codex;
    config_dir_env = "CODEX_HOME";
    config_dir_default = "~/.codex";
    config_file = "config.toml";
    forced = entriesOf (ix.attrs.flattenToDotted forcedSettings);
    soft = entriesOf (ix.attrs.flattenToDotted settings);
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
    rm -f $out/bin/${binName}
    makeBinaryWrapper ${launcher}/bin/config-launch $out/bin/${binName} \
      --inherit-argv0 \
      --set IX_LAUNCH_SPEC ${spec}
  '';
  meta = codex.meta // {
    description = "${codex.meta.description or "OpenAI Codex CLI"} (index wrapper with baked defaults)";
    mainProgram = binName;
  };
}
