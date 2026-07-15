{
  ix,
  lib,
  pkgs,
  paths,
}: let
  renderBuildMachine = import (paths.root + "/lib/nix/build-machine.nix") {
    inherit lib;
  };
  assertionsType = lib.types.listOf (lib.types.submodule {
    options = {
      assertion = lib.mkOption {type = lib.types.bool;};
      message = lib.mkOption {type = lib.types.str;};
    };
  });

  etcFileType = lib.types.submodule {
    options.text = lib.mkOption {type = lib.types.lines;};
  };

  optionStubs = {
    options = {
      assertions = lib.mkOption {
        type = assertionsType;
        default = [];
      };
      environment.etc = lib.mkOption {
        type = lib.types.attrsOf etcFileType;
        default = {};
      };
      environment.systemPackages = lib.mkOption {
        type = lib.types.listOf lib.types.package;
        default = [];
      };
      nix = {
        buildMachines = lib.mkOption {
          type = lib.types.listOf lib.types.raw;
          default = [];
        };
        distributedBuilds = lib.mkOption {
          type = lib.types.bool;
          default = false;
        };
        envVars = lib.mkOption {
          type = lib.types.attrsOf lib.types.str;
          default = {};
        };
        package = lib.mkOption {
          type = lib.types.nullOr lib.types.package;
          default = pkgs.nix;
        };
      };
    };
  };

  eval = extraModule:
    (lib.evalModules {
      modules = [
        optionStubs
        (import (paths.root + "/modules/darwin/remote-builders") {
          inherit renderBuildMachine;
          inherit (ix) writePythonApplication;
        })
        extraModule
      ];
    }).config;

  empty = eval {};
  configured = eval {
    nix.remoteBuilders = [
      {
        name = "vm-builder";
        hostName = "vm";
        sshKey = "/etc/nix/builder_ed25519";
        systems = ["aarch64-linux"];
        maxJobs = 8;
        supportedFeatures = [
          "big-parallel"
          "kvm"
        ];
        ssh = {
          connectTimeout = 7;
          strictHostKeyChecking = "accept-new";
          proxyCommand = "/run/current-system/sw/bin/vm-net-connect";
        };
      }
      {
        name = "cluster-builder";
        access = "protected";
        hostName = "cluster-builder.example.com";
        user = "builder";
        sshKey = "/etc/nix/cluster_ed25519";
        systems = ["x86_64-linux"];
        maxJobs = 32;
        speedFactor = 4;
        mandatoryFeatures = ["benchmark"];
        supportedFeatures = ["benchmark"];
        publicHostKey = "c3NoLWVkMjU1MTkgQUFBQQ==";
        ssh = {
          port = 9999;
          serverAliveInterval = 15;
          serverAliveCountMax = 12;
        };
      }
    ];
  };
  protectedOnly = eval {
    nix.remoteBuilders = [
      {
        name = "cluster-builder";
        access = "protected";
        hostName = "cluster-builder.example.com";
        sshKey = "/etc/nix/cluster_ed25519";
        systems = ["x86_64-linux"];
      }
    ];
  };
  missingNix = eval {
    nix = {
      package = null;
      remoteBuilders = [
        {
          name = "cluster-builder";
          access = "protected";
          hostName = "cluster-builder.example.com";
          systems = ["x86_64-linux"];
        }
      ];
    };
  };

  duplicate = eval {
    nix.remoteBuilders = [
      {
        name = "duplicate";
        hostName = "first.example.com";
        systems = ["aarch64-linux"];
      }
      {
        name = "duplicate";
        hostName = "second.example.com";
        systems = ["x86_64-linux"];
      }
    ];
  };
  storeKey = eval {
    nix.remoteBuilders = [
      {
        name = "store-key";
        hostName = "builder.example.com";
        sshKey = "${builtins.storeDir}/test-private-key";
        systems = ["aarch64-linux"];
      }
    ];
  };

  sshConfig = configured.environment.etc."ssh/ssh_config.d/000-index-remote-builders.conf".text;
  policy = builtins.fromJSON configured.environment.etc."nix/protected-builders.json".text;

  assertions = [
    {
      assertion = !empty.nix.distributedBuilds && empty.nix.buildMachines == [] && empty.environment.etc == {};
      message = "an empty nix.remoteBuilders list must leave distributed builds and SSH files untouched";
    }
    {
      assertion = configured.nix.distributedBuilds;
      message = "configured remote builders must enable distributed builds";
    }
    {
      assertion = configured.nix.envVars.NIX_SSHOPTS == "-F /etc/ssh/ssh_config";
      message = "remote builders must ignore root's mutable SSH config";
    }
    {
      assertion =
        configured.nix.buildMachines
        == [
          {
            hostName = "vm-builder";
            mandatoryFeatures = [];
            maxJobs = 8;
            protocol = "ssh-ng";
            publicHostKey = null;
            speedFactor = 1;
            sshKey = "/etc/nix/builder_ed25519";
            sshUser = "root";
            supportedFeatures = [
              "big-parallel"
              "kvm"
            ];
            systems = ["aarch64-linux"];
          }
        ];
      message = "only ambient builder records must map to nix.buildMachines";
    }
    {
      assertion =
        policy
        == {
          version = 1;
          maxTtlSeconds = 3600;
          cancelGraceSeconds = 5;
          verifyTimeoutSeconds = 30;
          builders.cluster-builder = {
            name = "cluster-builder";
            machine = "ssh-ng://builder@cluster-builder x86_64-linux /etc/nix/cluster_ed25519 32 4 benchmark benchmark c3NoLWVkMjU1MTkgQUFBQQ==";
          };
        };
      message = "protected builder records must render into the checked authorization policy";
    }
    {
      assertion =
        renderBuildMachine {
          hostName = "builder";
          mandatoryFeatures = [];
          maxJobs = 1;
          protocol = "ssh-ng";
          publicHostKey = null;
          speedFactor = 1;
          sshKey = null;
          sshUser = "root";
          supportedFeatures = [];
          systems = ["aarch64-linux"];
        }
        == "ssh-ng://root@builder aarch64-linux - 1 1 - - -";
      message = "the Nix build-machine renderer must preserve empty optional columns";
    }
    {
      assertion = builtins.length configured.environment.systemPackages == 1;
      message = "a protected builder must install exactly one authorization command";
    }
    {
      assertion =
        !protectedOnly.nix.distributedBuilds
        && protectedOnly.nix.buildMachines == []
        && protectedOnly.environment.etc ? "nix/protected-builders.json";
      message = "a protected-only configuration must fail closed for ambient Nix commands";
    }
    {
      assertion =
        missingNix.environment.systemPackages == []
        && lib.any (
          item: !item.assertion && item.message == "protected nix.remoteBuilders require nix.package"
        )
        missingNix.assertions;
      message = "protected builders without a Nix package must produce a failing module assertion";
    }
    {
      assertion =
        sshConfig
        == ''
          Host vm-builder
            BatchMode yes
            ConnectTimeout 7
            ControlMaster no
            ControlPath none
            ControlPersist no
            HostName vm
            IdentityAgent none
            IdentitiesOnly yes
            Port 22
            ProxyCommand /run/current-system/sw/bin/vm-net-connect
            StrictHostKeyChecking accept-new
            User root
            IdentityFile /etc/nix/builder_ed25519
          Host cluster-builder
            BatchMode yes
            ConnectTimeout 10
            ControlMaster no
            ControlPath none
            ControlPersist no
            HostName cluster-builder.example.com
            IdentityAgent none
            IdentitiesOnly yes
            Port 9999
            ProxyCommand none
            StrictHostKeyChecking yes
            User builder
            HostKeyAlias cluster-builder
            IdentityFile /etc/nix/cluster_ed25519
            ServerAliveInterval 15
            ServerAliveCountMax 12
        '';
      message = "the SSH stanzas must render fixed safeguards, strict identity defaults, and caller data";
    }
    {
      assertion = lib.any (item: !item.assertion && item.message == "nix.remoteBuilders names must be unique") duplicate.assertions;
      message = "duplicate builder aliases must produce a failing module assertion";
    }
    {
      assertion = lib.any (item: !item.assertion && item.message == "nix.remoteBuilders sshKey values must not point into the Nix store") storeKey.assertions;
      message = "store-backed private keys must produce a failing module assertion";
    }
  ];

  failures = map (assertion: assertion.message) (builtins.filter (assertion: !assertion.assertion) assertions);
in
  assert lib.assertMsg (failures == []) (
    "darwin-remote-builders:\n  " + lib.concatStringsSep "\n  " failures
  );
    pkgs.runCommand "ix-test-darwin-remote-builders" {
      __structuredAttrs = true;
      nativeBuildInputs = [pkgs.python3];
    } ''
      python ${paths.root + "/modules/darwin/remote-builders/test_protected_build.py"}
      mkdir -p "$out"
    ''
