{
  lib,
  writeShellApplication,
  # Pi is not yet packaged in this repo. Until the dependency-intake follow-up
  # lands a pinned `pi` derivation, pass null and the wrapper uses `pi` from PATH
  # (the dev image / system already provides it). Pass a derivation to pin it.
  pi ? null,
  # ix-mcp supplies the ONLY tool surface (python_exec + search_* + calendar_*).
  # Built from index/packages/mcp. Pass null to fall back to PATH for local dev.
  ix-mcp ? null,
}:
let
  models = import ./models.nix;
  defaultModel = "claude";

  # Generate the provider/model lookup as a bash case from the declarative table,
  # so models.nix stays the single source of truth.
  modelCase = lib.concatStringsSep "\n" (
    lib.mapAttrsToList (
      alias: m: "    ${alias}) provider=${m.provider}; model=${m.model} ;;"
    ) models
  );

  runtimeInputs = lib.optional (pi != null) pi ++ lib.optional (ix-mcp != null) ix-mcp;
in
writeShellApplication {
  name = "pi-harness";
  inherit runtimeInputs;
  # The bridge extension source travels with the wrapper; Pi loads it with -e.
  # node_modules for @modelcontextprotocol/sdk are resolved at dev time today
  # (see smoke/run.sh); making that pure via buildNpmPackage is a follow-up.
  text = ''
    # Pi engine harness (ENG-2262): launch Pi as a Room-facing engine with the
    # built-in tools ABSENT (--no-builtin-tools), exposing only the ix-mcp tool
    # surface through the bridge extension, and emitting a machine-readable JSON
    # event stream. Model selection is declarative (models.nix); API keys come
    # from the environment the caller hands us, never looked up here.

    model_alias="''${PI_HARNESS_MODEL:-${defaultModel}}"
    provider=""
    model=""
    case "$model_alias" in
${modelCase}
      *) echo "pi-harness: unknown model alias '$model_alias'" >&2; exit 2 ;;
    esac

    # Minimal, controlled system prompt by default - no accidental repo-wide
    # instructions. Override with PI_HARNESS_SYSTEM_PROMPT for a richer agent.
    system_prompt="''${PI_HARNESS_SYSTEM_PROMPT:-You are a coding agent. All actions - shell, file IO, HTTP - run through the python_exec tool on a shared Python kernel.}"

    # --mode json: stable JSON event stream for Room (default). text/rpc available
    # via PI_HARNESS_MODE for interactive dev.
    mode="''${PI_HARNESS_MODE:-json}"

    exec pi \
      --no-builtin-tools \
      --no-extensions \
      --no-skills \
      --no-session \
      --mode "$mode" \
      --print \
      --provider "$provider" \
      --model "$model" \
      --system-prompt "$system_prompt" \
      -e ${./extension}/ix-mcp-bridge.ts \
      "$@"
  '';

  meta = {
    description = "Pi engine harness: Pi with built-in tools absent, exposing only the ix-mcp surface, emitting a JSON event stream for Room";
    mainProgram = "pi-harness";
  };
}
