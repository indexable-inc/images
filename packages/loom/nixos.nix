{
  lib,
  pkgs,
  loomPackage,
  ...
}: let
  ixCli = loomPackage.passthru.ixCli;
  loomClaudeSource = pkgs.replaceVars ./loom-claude.sh {
    claude = lib.getExe pkgs.claude-code;
  };
  loomClaude = pkgs.runCommand "loom-claude" {strictDeps = true;} ''
    install -Dm755 ${loomClaudeSource} "$out/bin/loom-claude"
  '';
in {
  nixpkgs.config.allowUnfreePredicate = package: lib.getName package == "claude-code";

  networking.hostName = "loom";

  environment = {
    systemPackages = [
      loomPackage
      ixCli
      loomClaude
    ];
    variables = {
      LOOM_PARENT_VM = "loom";
      LOOM_IX_BIN = lib.getExe ixCli;
      LOOM_CLAUDE_BIN = lib.getExe' loomClaude "loom-claude";
      LOOM_PREFLIGHT = "test -s /var/lib/loom/anthropic_api_key";
      LOOM_CLAUDE_ARGS = "--dangerously-skip-permissions";
    };
  };

  systemd = {
    tmpfiles.rules = [
      "d /var/lib/loom 0700 root root -"
      "d /root/.config/ix 0700 root root -"
    ];
    services.loom-credentials = {
      description = "Persist Loom credentials for snapshot forks";
      wantedBy = ["multi-user.target"];
      unitConfig.ConditionPathExists = [
        "/run/secrets/anthropic_api_key"
        "/run/secrets/loom_ix_token"
      ];
      path = [pkgs.coreutils];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        UMask = "0077";
      };
      script = ''
        install -m600 /run/secrets/anthropic_api_key /var/lib/loom/anthropic_api_key
        printf 'token = "%s"\nserver = "https://api.ix.dev"\n' \
          "$(cat /run/secrets/loom_ix_token)" > /root/.config/ix/config.toml
      '';
    };
  };

  system.stateVersion = "26.05";
}
