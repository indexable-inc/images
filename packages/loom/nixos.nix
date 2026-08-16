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
      # The `loom` wrapper carries its own pinned zellij; this one is for
      # operating the session from outside it (list-sessions, kill-session).
      pkgs.zellij
    ];
    variables = {
      IS_SANDBOX = "1";
      LOOM_PARENT_VM = "loom";
      LOOM_IX_BIN = lib.getExe ixCli;
      LOOM_CLAUDE_BIN = lib.getExe' loomPackage "loom-claude";
      LOOM_REMOTE_CLAUDE_BIN = lib.getExe' loomPackage "loom-remote-claude";
      # Gate the fork child on the interior actually being ready: the key
      # file materialized AND the claude launcher resolvable. A freshly
      # restored fork hydrates its store lazily, and a child launched before
      # loom-claude resolves dies as an opaque exit (measured live in the
      # template e2e: PATH lookup failed seconds after restore, then
      # succeeded).
      LOOM_PREFLIGHT = "test -s /var/lib/loom/anthropic_api_key && command -v loom-claude";
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
