# Claude Code's managed-settings layer, declared by Nix on darwin.
#
# Managed settings are the only Claude Code scope that cannot be overridden:
# they outrank project, user AND command-line settings, and permission arrays
# union across scopes, so a deny declared here can never be allowed back by a
# runtime write or a hand edit. That is why policy lives here rather than in
# ~/.claude/settings.json, which Claude Code rewrites from memory on any /model
# or /config toggle -- every Nix owner of that file was reduced to racing the
# app between switches (#4312).
#
# Root-owned system scope, so this is a nix-darwin module rather than a Home
# Manager one; Home Manager cannot write under /Library at all. Feed it the
# wrapper's render:
#
#   programs.claude-code.managedSettings = {
#     enable = true;
#     settings = self.homeConfigurations.me.config.programs.claude-code.finalPackage.passthru.settingsPolicy;
#   };
#
# The NixOS equivalent needs no module: environment.etc can own
# /etc/claude-code/managed-settings.json directly (see lib/dev/agents.nix).
{
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.programs.claude-code.managedSettings;
  jsonFormat = pkgs.formats.json {};
  rendered = jsonFormat.generate "claude-code-managed-settings.json" cfg.settings;
  # Documented macOS location, and the only file-based managed source Claude
  # Code reads besides the managed-settings.d/ directory beside it.
  # https://code.claude.com/docs/en/settings.md
  directory = "/Library/Application Support/ClaudeCode";
  target = "${directory}/managed-settings.json";
  dropIn = "${directory}/managed-settings.d";
in {
  options.programs.claude-code.managedSettings = {
    enable = lib.mkEnableOption "the Nix-declared Claude Code managed settings policy";

    settings = lib.mkOption {
      inherit (jsonFormat) type;
      default = {};
      example = lib.literalExpression "claudeCode.passthru.settingsPolicy";
      description = ''
        Policy written to {file}`${target}`, read-only and root-owned. Every
        key here is unreachable from a user, project or CLI scope, so declare
        posture (hooks, `permissions.deny`, `env`, `statusLine`) and never the
        preferences Claude Code writes back itself (`theme`, `verbose`,
        `model`) -- one of those declared here is a toggle the user can no
        longer move.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        # An empty render would deploy a file that enforces nothing while
        # reading as though policy were in force.
        assertion = cfg.settings != {};
        message = "programs.claude-code.managedSettings.enable is on with no settings; give it the wrapper's passthru.settingsPolicy or turn it off.";
      }
    ];

    system.activationScripts.postActivation.text = lib.mkAfter ''
      ${pkgs.coreutils}/bin/install -d -m 0755 ${lib.escapeShellArg directory}
      # 0444: Claude Code only ever reads this file, and a writable copy would
      # reintroduce exactly the drift the managed layer exists to prevent.
      if ! ${pkgs.diffutils}/bin/cmp -s ${rendered} ${lib.escapeShellArg target}; then
        ${pkgs.coreutils}/bin/install -m 0444 ${rendered} ${lib.escapeShellArg target}
        echo "wrote Claude Code managed settings -> ${target}"
      fi
      # A second file-based managed source silently outranks or merges with
      # this one. Not fatal: on an MDM-managed Mac such a drop-in is
      # legitimate and deliberately outranks local policy, so name it and let
      # the operator judge rather than refusing to activate.
      if [ -d ${lib.escapeShellArg dropIn} ] && [ -n "$(${pkgs.coreutils}/bin/ls -A ${lib.escapeShellArg dropIn} 2>/dev/null)" ]; then
        echo "warning: ${dropIn} is non-empty; those files also apply as managed settings and may override this policy" >&2
      fi
    '';
  };
}
