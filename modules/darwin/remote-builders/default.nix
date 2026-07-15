{
  renderBuildMachine,
  writePythonApplication,
}: {
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.nix.remoteBuilders;
  protectedCfg = config.nix.protectedBuilders;

  tokenType = lib.types.strMatching "[^ \t\r\n]+";
  commandType = lib.types.strMatching "[^\r\n]+";
  hostType = lib.types.strMatching "[A-Za-z0-9._:%-]+";
  keyType = lib.types.strMatching "/[^ \t\r\n]+";
  userType = lib.types.strMatching "[A-Za-z0-9._-]+";

  builderType = lib.types.submodule {
    options = {
      name = lib.mkOption {
        type = lib.types.strMatching "[A-Za-z0-9][A-Za-z0-9._-]*";
        example = "linux-builder";
        description = "SSH alias used by Nix for this builder.";
      };

      hostName = lib.mkOption {
        type = hostType;
        example = "builder.example.com";
        description = "Network host passed to OpenSSH for this builder.";
      };

      user = lib.mkOption {
        type = userType;
        default = "root";
        description = "Remote SSH user.";
      };

      access = lib.mkOption {
        type = lib.types.enum [
          "ambient"
          "protected"
        ];
        default = "ambient";
        description = ''
          Whether ordinary Nix commands may schedule this builder. Protected
          builders are available only through protected-nix-build.
        '';
      };

      sshKey = lib.mkOption {
        type = lib.types.nullOr keyType;
        default = null;
        example = "/etc/nix/builder_ed25519";
        description = "Local private key used by Nix and OpenSSH, or null to use a default identity file.";
      };

      systems = lib.mkOption {
        type = lib.types.listOf tokenType;
        example = ["aarch64-linux"];
        description = "Nix systems this builder can execute.";
      };

      protocol = lib.mkOption {
        type = lib.types.enum [
          "ssh"
          "ssh-ng"
        ];
        default = "ssh-ng";
        description = "Nix remote-store protocol.";
      };

      maxJobs = lib.mkOption {
        type = lib.types.ints.positive;
        default = 1;
        description = "Maximum concurrent jobs scheduled on this builder.";
      };

      speedFactor = lib.mkOption {
        type = lib.types.ints.positive;
        default = 1;
        description = "Relative scheduling preference for this builder.";
      };

      supportedFeatures = lib.mkOption {
        type = lib.types.listOf tokenType;
        default = [];
        example = [
          "big-parallel"
          "kvm"
        ];
        description = "Nix system features available on this builder.";
      };

      mandatoryFeatures = lib.mkOption {
        type = lib.types.listOf tokenType;
        default = [];
        description = "Features a derivation must require before Nix uses this builder.";
      };

      publicHostKey = lib.mkOption {
        type = lib.types.nullOr tokenType;
        default = null;
        description = "Base64-encoded SSH host key recorded in Nix's machine entry.";
      };

      ssh = {
        port = lib.mkOption {
          type = lib.types.port;
          default = 22;
          description = "SSH port.";
        };

        connectTimeout = lib.mkOption {
          type = lib.types.ints.positive;
          default = 10;
          description = "SSH connection timeout in seconds.";
        };

        strictHostKeyChecking = lib.mkOption {
          type = lib.types.enum [
            "yes"
            "accept-new"
          ];
          default = "yes";
          description = "OpenSSH host-key policy. The default requires a pre-provisioned known-hosts entry.";
        };

        proxyCommand = lib.mkOption {
          type = lib.types.nullOr commandType;
          default = null;
          example = "/run/current-system/sw/bin/connect-builder";
          description = "Single-line OpenSSH ProxyCommand, or null for a direct connection.";
        };

        serverAliveInterval = lib.mkOption {
          type = lib.types.nullOr lib.types.ints.positive;
          default = null;
          description = "Seconds between SSH keepalive messages, or null to use the OpenSSH default.";
        };

        serverAliveCountMax = lib.mkOption {
          type = lib.types.nullOr lib.types.ints.positive;
          default = null;
          description = "Unanswered SSH keepalives allowed before disconnecting, or null to use the OpenSSH default.";
        };
      };
    };
  };

  machineFor = builder: {
    hostName = builder.name;
    sshUser = builder.user;
    inherit
      (builder)
      mandatoryFeatures
      maxJobs
      protocol
      publicHostKey
      speedFactor
      sshKey
      supportedFeatures
      systems
      ;
  };

  sshSettingsFor = builder:
    [
      {
        name = "BatchMode";
        value = "yes";
      }
      {
        name = "ConnectTimeout";
        value = builder.ssh.connectTimeout;
      }
      {
        name = "ControlMaster";
        value = "no";
      }
      {
        name = "ControlPath";
        value = "none";
      }
      {
        name = "ControlPersist";
        value = "no";
      }
      {
        name = "HostName";
        value = builder.hostName;
      }
      {
        name = "IdentityAgent";
        value = "none";
      }
      {
        name = "IdentitiesOnly";
        value = "yes";
      }
      {
        name = "Port";
        value = builder.ssh.port;
      }
      {
        # "none" pins direct builders to a direct connection; without it a
        # later Host * ProxyCommand from the system config would still apply.
        name = "ProxyCommand";
        value =
          if builder.ssh.proxyCommand != null
          then builder.ssh.proxyCommand
          else "none";
      }
      {
        name = "StrictHostKeyChecking";
        value = builder.ssh.strictHostKeyChecking;
      }
      {
        name = "User";
        value = builder.user;
      }
    ]
    ++ lib.optional (builder.publicHostKey != null) {
      # Nix records publicHostKey under the alias, but OpenSSH looks keys up
      # by the rewritten HostName; alias the lookup so the declared key wins.
      name = "HostKeyAlias";
      value = builder.name;
    }
    ++ lib.optional (builder.sshKey != null) {
      name = "IdentityFile";
      value = builder.sshKey;
    }
    ++ lib.optional (builder.ssh.serverAliveInterval != null) {
      name = "ServerAliveInterval";
      value = builder.ssh.serverAliveInterval;
    }
    ++ lib.optional (builder.ssh.serverAliveCountMax != null) {
      name = "ServerAliveCountMax";
      value = builder.ssh.serverAliveCountMax;
    };

  renderSshConfig = builder:
    lib.concatStringsSep "\n" (
      ["Host ${builder.name}"]
      ++ map (setting: "  ${setting.name} ${toString setting.value}") (sshSettingsFor builder)
    )
    + "\n";

  builderNames = map (builder: builder.name) cfg;
  ambientBuilders = lib.filter (builder: builder.access == "ambient") cfg;
  protectedBuilders = lib.filter (builder: builder.access == "protected") cfg;
  policyPath = "/etc/nix/protected-builders.json";
  protectedNixBuild = writePythonApplication pkgs {
    name = "protected-nix-build";
    src = ./protected_build.py;
    args = [
      policyPath
      (lib.getExe config.nix.package)
      "/usr/bin/logger"
    ];
    meta.description = "Run a time-bounded Nix build on explicit protected builders";
  };
in {
  options.nix = {
    remoteBuilders = lib.mkOption {
      type = lib.types.listOf builderType;
      default = [];
      example = lib.literalExpression ''
        [
          {
            name = "linux-builder";
            hostName = "builder.example.com";
            user = "root";
            sshKey = "/etc/nix/builder_ed25519";
            systems = [ "aarch64-linux" ];
            maxJobs = 8;
            supportedFeatures = [ "big-parallel" "kvm" ];
          }
        ]
      '';
      description = ''
        Remote Nix builders. Ambient records produce nix.buildMachines
        entries. Every record produces a protocol-safe system OpenSSH stanza.
      '';
    };

    protectedBuilders = {
      maxTtlSeconds = lib.mkOption {
        type = lib.types.ints.between 1 86400;
        default = 3600;
        description = "Maximum duration of one protected build authorization.";
      };

      cancelGraceSeconds = lib.mkOption {
        type = lib.types.ints.between 1 60;
        default = 5;
        description = "Grace period between terminating and killing the Nix client process group.";
      };

      verifyTimeoutSeconds = lib.mkOption {
        type = lib.types.ints.between 1 300;
        default = 30;
        description = "Time allowed for cancelled work to disappear from the daemon build status.";
      };
    };
  };

  config = lib.mkIf (cfg != []) {
    assertions = [
      {
        assertion = builtins.length builderNames == builtins.length (lib.unique builderNames);
        message = "nix.remoteBuilders names must be unique";
      }
      {
        assertion = lib.all (builder: builder.systems != []) cfg;
        message = "every nix.remoteBuilders entry must declare at least one system";
      }
      {
        assertion = lib.all (builder: builder.sshKey == null || !lib.hasPrefix "${builtins.storeDir}/" builder.sshKey) cfg;
        message = "nix.remoteBuilders sshKey values must not point into the Nix store";
      }
      {
        assertion = protectedBuilders == [] || config.nix.package != null;
        message = "protected nix.remoteBuilders require nix.package";
      }
    ];

    nix = {
      buildMachines = map machineFor ambientBuilders;
      distributedBuilds = ambientBuilders != [];
      envVars.NIX_SSHOPTS = lib.escapeShellArgs [
        "-F"
        "/etc/ssh/ssh_config"
      ];
    };

    # OpenSSH keeps the first value read for each scalar option. Sort these
    # host-specific invariants before other system snippets so a Host * block
    # cannot silently re-enable multiplexing for Nix protocol streams.
    environment.etc."ssh/ssh_config.d/000-index-remote-builders.conf".text =
      lib.concatMapStrings renderSshConfig cfg;

    environment.etc."nix/protected-builders.json" = lib.mkIf (protectedBuilders != []) {
      text = builtins.toJSON {
        version = 1;
        inherit
          (protectedCfg)
          cancelGraceSeconds
          maxTtlSeconds
          verifyTimeoutSeconds
          ;
        builders = lib.listToAttrs (
          map (builder:
            lib.nameValuePair builder.name {
              inherit (builder) name;
              machine = renderBuildMachine (machineFor builder);
            })
          protectedBuilders
        );
      };
    };

    environment.systemPackages = lib.optional (
      protectedBuilders != [] && config.nix.package != null
    ) protectedNixBuild;
  };
}
