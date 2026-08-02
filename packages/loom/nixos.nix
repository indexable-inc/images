{
  lib,
  pkgs,
  loomPackage,
  ...
}: let
  ixCli = loomPackage.passthru.ixCli;
in {
  nixpkgs.config.allowUnfreePredicate = package: lib.getName package == "claude-code";

  networking.hostName = "loom";

  environment = {
    etc."claude-code/managed-settings.json".source =
      loomPackage.passthru.claudeCode.passthru.settingsPolicyFile;
    systemPackages = [
      loomPackage
      ixCli
      loomPackage.passthru.claudeCode
      loomPackage.passthru.mcp
    ];
    variables = {
      IS_SANDBOX = "1";
      LOOM_PARENT_VM = "loom";
      LOOM_IX_BIN = lib.getExe ixCli;
      LOOM_CLAUDE_BIN = lib.getExe' loomPackage "loom-claude";
      LOOM_REMOTE_CLAUDE_BIN = lib.getExe' loomPackage "loom-remote-claude";
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
