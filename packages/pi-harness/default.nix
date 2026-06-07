{
  lib,
  writeNushellApplication,
  # Pi is not yet packaged in this repo. Until the dependency-intake follow-up
  # lands a pinned `pi` derivation, the wrapper calls `pi` from PATH (the dev
  # image / system already provides it). Pass a derivation here to pin it.
  pi ? null,
  # ix-mcp supplies the ONLY tool surface (python_exec + search_* + calendar_*),
  # built from index/packages/mcp. Pass null to fall back to PATH for local dev.
  ix-mcp ? null,
}:
let
  models = import ./models.nix;
  defaultModel = "claude";

  # Render the declarative model table (models.nix) as a Nushell record literal,
  # so models.nix stays the single source of truth for provider/model selection.
  modelTable = lib.concatStringsSep ", " (
    lib.mapAttrsToList (
      alias: m: ''"${alias}": { provider: "${m.provider}", model: "${m.model}" }''
    ) models
  );

  # The bridge source travels with the wrapper; Pi loads it with --extension.
  # Name exactly the files the build needs (the manifests are for the future
  # buildNpmPackage follow-up), so node_modules/_probe never enter the closure.
  extension = lib.fileset.toSource {
    root = ./extension;
    fileset = lib.fileset.unions [
      (./extension + "/ix-mcp-bridge.ts")
      (./extension + "/package.json")
      (./extension + "/package-lock.json")
    ];
  };

  runtimeInputs = lib.optional (pi != null) pi ++ lib.optional (ix-mcp != null) ix-mcp;
in
writeNushellApplication {
  name = "pi-harness";
  inherit runtimeInputs;
  text = ''
    # Pi engine harness (ENG-2262): run Pi as a Room-facing engine with the
    # built-in tools ABSENT (--no-builtin-tools), exposing only the ix-mcp tool
    # surface via the bridge extension, and emitting a JSON event stream. Model
    # selection is declarative (models.nix); API keys come from the caller's
    # environment, never looked up here.
    def main [...rest] {
      let alias = ($env.PI_HARNESS_MODEL? | default "${defaultModel}")
      let table = { ${modelTable} }
      let cfg = ($table | get --ignore-errors $alias)
      if ($cfg == null) {
        print --stderr $"pi-harness: unknown model alias '($alias)'"
        exit 2
      }

      # Minimal, controlled system prompt by default - no accidental repo-wide
      # instructions. Override with PI_HARNESS_SYSTEM_PROMPT for a richer agent.
      let system_prompt = (
        $env.PI_HARNESS_SYSTEM_PROMPT?
        | default "You are a coding agent. All actions - shell, file IO, HTTP - run through the python_exec tool on a shared Python kernel."
      )

      # --mode json: stable JSON event stream for Room (default). text/rpc are
      # available via PI_HARNESS_MODE for interactive dev.
      let mode = ($env.PI_HARNESS_MODE? | default "json")

      (
        ^pi
        --no-builtin-tools --no-extensions --no-skills --no-session
        --mode $mode --print
        --provider $cfg.provider --model $cfg.model
        --system-prompt $system_prompt
        --extension ${extension}/ix-mcp-bridge.ts
        ...$rest
      )
    }
  '';

  meta = {
    description = "Pi engine harness: Pi with built-in tools absent, exposing only the ix-mcp surface, emitting a JSON event stream for Room";
    mainProgram = "pi-harness";
  };
}
