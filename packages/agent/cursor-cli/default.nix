{
  lib,
  ix,
  cursor-cli,
  makeBinaryWrapper,
  symlinkJoin,
  formats,
  binName ? "cursor-agent",
  # Sibling repo packages from the flake package set (threaded by
  # lib/packages.nix), used to locate the `ix-mcp` entrypoint for the rendered
  # `index` MCP server. `{ }` when the `mcp` sibling is out of scope, in which
  # case only the keyless `exa` server is rendered.
  repoPackages ? {},
  # Rule names dropped from the default house prompt. Only affects the computed
  # `systemPrompt` default below; ignored when `systemPrompt` is passed
  # explicitly.
  omitRules ? [],
  # Bake `--force` ("force allow commands unless explicitly denied") so every
  # session starts without per-command approval dialogs: the same posture as
  # claude-code's default `--dangerously-skip-permissions`, for the same reason
  # (trusted config in disposable sandboxes where approval prompts only stall
  # the agent). `cli-config.json` deny rules still apply; `--force` skips the
  # allowlist prompt, not explicit denies.
  forceAllowCommands ? true,
  # The shared MCP server set, rendered to `passthru.mcpJson` rather than baked
  # into argv: cursor-agent has no `--mcp-config`-style flag, so the global
  # `~/.cursor/mcp.json` is the only injection point and delivery belongs to
  # the consumer's config management (home-manager already owns that file).
  mcpServers ?
    (import (ix.paths.packagesRoot + "/agent/common.nix") {
      inherit lib ix repoPackages;
      promptOmitRules = omitRules;
    }).defaultServers,
  # The house prompt rendered for the cursor runtime. Cursor has no
  # system-prompt flag either (rules load from a project's AGENTS.md /
  # `.cursor/rules`), so this too ships as passthru for config delivery. Null
  # renders no passthru prompt file.
  systemPrompt ?
    (import (ix.paths.packagesRoot + "/agent/common.nix") {
      inherit lib ix repoPackages;
      promptOmitRules = omitRules;
    }).systemPromptFor
    "cursor",
}: let
  # nixpkgs tags the vendored binary `licenses.unfree`, and the per-system
  # flake package set evaluates nixpkgs without `allowUnfree`, so consuming
  # `cursor-cli` as-is would block `nix run .#cursor-cli`. Same posture as
  # claude-code: omit the license marker for the vendored binary rather than
  # gate the output; distribution terms are Cursor's commercial license.
  cursorAgent = cursor-cli.overrideAttrs (previousAttrs: {
    meta = builtins.removeAttrs previousAttrs.meta ["license"];
  });

  sharedPermissions = import (ix.paths.packagesRoot + "/agent/policy/permissions.nix") {
    inherit lib;
    indexKernelBaked = mcpServers ? index;
    exaSearchBaked = mcpServers ? exa;
  };

  wrapperFlags = lib.optional forceAllowCommands "--force";
in
  symlinkJoin {
    name = "cursor-cli-${cursorAgent.version}";
    paths = [cursorAgent];
    # symlinkJoin links the whole cursor-cli output (the share/cursor-agent
    # payload tree); we only replace the bin entrypoint with our wrapper so the
    # baked flags ride every invocation while everything else stays pristine.
    nativeBuildInputs = [makeBinaryWrapper];
    postBuild = ''
      # shell
      rm -f $out/bin/${binName}
      makeBinaryWrapper ${cursorAgent}/share/cursor-agent/cursor-agent $out/bin/${binName} \
        --inherit-argv0 ${lib.concatMapStringsSep " " (flag: "--add-flags ${flag}") wrapperFlags}
    '';
    passthru =
      {
        inherit mcpServers;
        # Rendered `~/.cursor/mcp.json` content (the shared registry in
        # cursor's schema) for a consumer to deliver; see the `mcpServers`
        # comment for why this is passthru instead of a baked flag.
        mcpJson = (formats.json {}).generate "cursor-mcp.json" {
          mcpServers = ix.mcp.toCursorJson mcpServers;
        };
        permissions = sharedPermissions.cursor;
      }
      // lib.optionalAttrs (systemPrompt != null) {
        inherit systemPrompt;
        systemPromptFile = builtins.toFile "cursor-system-prompt.md" systemPrompt;
      };
    meta =
      cursorAgent.meta
      // {
        description = "${cursorAgent.meta.description or "Cursor CLI"} (index wrapper with baked defaults)";
        mainProgram = binName;
      };
  }
