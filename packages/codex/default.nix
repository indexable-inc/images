{
  lib,
  codex,
  makeBinaryWrapper,
  symlinkJoin,
  binName ? "codex",
  # Config-key defaults, applied as highest-precedence runtime `-c key=value`
  # overrides (codex's `--config` layer wins over every file layer, so a baked
  # value here cannot be silently dropped by a churned ~/.codex/config.toml).
  # Declared as data so the flags below derive from one source; a consumer adds
  # or replaces keys with `codex.override { settings = { ... }; }`.
  #
  # We run a trusted config (our own AGENTS.md / hooks / MCP servers), so the
  # one default we bake fleet-wide is turning off the startup update check: the
  # store binary is read-only and the wrapper owns the version pin, so the check
  # only ever costs a network round-trip it can never act on. Anything
  # security-shaped (sandbox mode, approval policy) is left to the user's config
  # and codex's own requirements layer, since a bare host is genuinely not a
  # sandbox.
  settings ? {
    check_for_update_on_startup = false;
  },
}:
let
  # Render an attrset of config defaults into the repeated `--config key=value`
  # flags codex accepts. Each value is encoded as TOML (booleans bare, strings
  # quoted, tables inline) so structured defaults round-trip through the runtime
  # override layer exactly as a config.toml entry would. Inline tables are
  # emitted WITHOUT spaces so the whole flag survives makeBinaryWrapper's
  # space-splitting of `--add-flags`.
  toToml =
    value:
    if builtins.isBool value then
      (if value then "true" else "false")
    else if builtins.isString value then
      builtins.toJSON value
    else if builtins.isInt value || builtins.isFloat value then
      toString value
    else if builtins.isAttrs value then
      "{${lib.concatStringsSep "," (lib.mapAttrsToList (k: v: "${k}=${toToml v}") value)}}"
    else
      throw "codex: unsupported config value type for ${builtins.toJSON value}";
  configFlags = lib.mapAttrsToList (key: value: "--config ${key}=${toToml value}") settings;
in
symlinkJoin {
  name = "codex-${codex.version}";
  paths = [ codex ];
  nativeBuildInputs = [ makeBinaryWrapper ];
  # symlinkJoin links the whole codex output (libexec, completions, ...); we only
  # re-wrap the entrypoint so our baked `-c` defaults ride every invocation while
  # everything else stays pristine. `--add-flags` (prepended) keeps a user's
  # explicit `-c` on the CLI winning, since codex is last-wins within the runtime
  # layer.
  postBuild = ''
    rm -f $out/bin/${binName}
    makeBinaryWrapper ${lib.getExe codex} $out/bin/${binName} \
      --inherit-argv0 \
      --add-flags ${lib.escapeShellArg (lib.concatStringsSep " " configFlags)}
  '';
  meta = codex.meta // {
    description = "${codex.meta.description or "OpenAI Codex CLI"} (index wrapper with baked defaults)";
    mainProgram = binName;
  };
}
