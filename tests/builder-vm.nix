# Eval test for the builder-vm pair (#2990): the darwin host module's tool
# set, closure boundary (systemPackages never reference the guest closure),
# remote-builder record, and launchd daemon, plus a full nixosSystem eval of
# the guest appliance module. Eval-only except the tool scripts, which are
# built so writeBashApplication's `bash -n` + shellcheck gates run in CI.
{
  lib,
  pkgs,
  ix,
  paths,
  nixpkgs,
}: let
  # The host module only ever evaluates against darwin pkgs (nix-darwin), and
  # its `vm` runner embeds vfkit, which nixpkgs marks darwin-only and refuses
  # to evaluate for linux. Eval it the way its real audience does.
  darwinPkgs = import nixpkgs {
    system = "aarch64-darwin";
    config = {};
    overlays = [
      (final: _: {
        nixos-rebuild-ng = ix.writeBashApplication final {
          name = "nixos-rebuild";
          text = ''
            : "''${VM_DEPLOY_ARGS:?}"
            printf '%s\n' "$@" > "$VM_DEPLOY_ARGS"
          '';
        };
      })
    ];
  };
  deployFlake = "/tmp/index builder?ref=main&rev=1#vm";
  spec = {
    mac = "5a:94:ef:2b:17:0c";
    cpus = 8;
    memMiB = 16 * 1024;
    diskGiB = 256;
  };

  # The nix-darwin option surface the host module writes into, stubbed so the
  # eval needs no darwin host.
  optionStubs = {
    options = {
      environment.systemPackages = lib.mkOption {
        type = lib.types.listOf lib.types.package;
        default = [];
      };
      launchd.daemons = lib.mkOption {
        type = lib.types.attrsOf lib.types.raw;
        default = {};
      };
      assertions = lib.mkOption {
        type = lib.types.listOf lib.types.raw;
        default = [];
      };
    };
  };

  evalHost = extraModule:
    (lib.evalModules {
      modules = [
        {_module.args.pkgs = darwinPkgs;}
        optionStubs
        (import (paths.modules + "/darwin/builder-vm.nix") {
          inherit (ix) writeBashApplication;
        })
        extraModule
      ];
    }).config;

  enabled = evalHost {
    services.builder-vm = spec // {enable = true;};
  };

  withGuest = evalHost {
    services.builder-vm =
      spec
      // {
        enable = true;
        guest = {
          image = darwinPkgs.emptyDirectory;
          imageFileName = "vm.raw";
        };
        deploy.flake = deployFlake;
      };
  };

  disabled = evalHost {
    services.builder-vm = spec;
  };

  partialGuest = evalHost {
    services.builder-vm =
      spec
      // {
        guest.image = darwinPkgs.emptyDirectory;
      };
  };

  builder = enabled.services.builder-vm.remoteBuilder;
  daemon = enabled.launchd.daemons.vm-builder.serviceConfig;

  guest =
    (lib.nixosSystem {
      system = "aarch64-linux";
      modules = [
        (paths.modules + "/services/builder-vm")
        {
          system.stateVersion = "25.05";
          services.builder-vm = {
            enable = true;
            builderPublicKey = "ssh-ed25519 AAAATESTKEY builder@test";
          };
        }
      ];
    }).config;

  assertions = [
    {
      assertion =
        lib.attrNames enabled.services.builder-vm.packages == ["vm" "vm-net-connect" "vm-ssh"];
      message = "without guest.image/deploy.flake, packages should be exactly vm, vm-net-connect, vm-ssh";
    }
    {
      assertion =
        lib.attrNames withGuest.services.builder-vm.packages
        == ["vm" "vm-deploy" "vm-install" "vm-net-connect" "vm-ssh"];
      message = "guest.image and deploy.flake should add vm-install and vm-deploy to packages";
    }
    {
      assertion = map lib.getName enabled.environment.systemPackages == ["vm" "vm-ssh"];
      message = "systemPackages should carry only the guest-closure-free tools (vm, vm-ssh)";
    }
    {
      assertion = daemon.Label == "org.nixos.vm-builder" && daemon.KeepAlive && daemon.RunAtLoad;
      message = "the launchd daemon should keep the guest always on";
    }
    {
      assertion = daemon.EnvironmentVariables.VM_STATE_DIR == "/var/lib/vm-builder";
      message = "the daemon should pin VM_STATE_DIR to the configured state dir";
    }
    {
      assertion = lib.hasSuffix "/bin/vm" (builtins.head daemon.ProgramArguments);
      message = "the daemon should run the vm runner";
    }
    {
      assertion = builder.name == "vm-builder" && builder.hostName == "vm";
      message = "the remote-builder record should keep the vm-builder/vm alias split (ssh multiplexing hazard)";
    }
    {
      assertion = builder.protocol == "ssh-ng";
      message = "the forced nix-daemon --stdio builder key requires the ssh-ng protocol";
    }
    {
      assertion = builder.systems == ["aarch64-linux"] && builder.maxJobs == spec.cpus;
      message = "the remote-builder record should advertise aarch64-linux with one job per vCPU";
    }
    {
      assertion = !(lib.elem "kvm" builder.supportedFeatures);
      message = "the vfkit guest has no /dev/kvm; kvm must not be advertised by default";
    }
    {
      assertion = lib.hasSuffix "/bin/vm-net-connect" builder.ssh.proxyCommand;
      message = "the remote-builder record should reach the guest through vm-net-connect";
    }
    {
      assertion = disabled.environment.systemPackages == [] && disabled.launchd.daemons == {};
      message = "the disabled host module should stay inert";
    }
    {
      assertion =
        !(partialGuest.services.builder-vm.packages ? vm-install)
        && lib.any (
          item:
            !item.assertion
            && item.message
            == "services.builder-vm.guest.image and services.builder-vm.guest.imageFileName must be set together"
        )
        partialGuest.assertions;
      message = "a partial guest image configuration should produce a clear assertion without evaluating vm-install";
    }
    {
      assertion =
        guest.fileSystems."/".device
        == "/dev/disk/by-partlabel/root"
        && guest.fileSystems."/".autoResize;
      message = "the guest root should mount by GPT partlabel and grow its filesystem";
    }
    {
      assertion =
        guest.boot.initrd.systemd.repart.enable && guest.boot.initrd.systemd.repart.device == "/dev/vda";
      message = "the guest initrd should repart-grow the vfkit virtio disk";
    }
    {
      assertion = guest.systemd.services ? first-boot-registration;
      message = "the guest should register the installed image as a generation on first boot";
    }
    {
      assertion =
        guest.image.repart.partitions
        ? esp
        && guest.image.repart.partitions.root.repartConfig.Minimize == "guess";
      message = "the install image should carry an ESP and a minimized root partition";
    }
    {
      assertion =
        lib.any (
          key: lib.hasPrefix ''restrict,command="'' key && lib.hasSuffix "builder@test" key
        )
        guest.users.users.root.openssh.authorizedKeys.keys;
      message = "the builder public key should be authorized restricted to the nix-daemon protocol";
    }
    {
      assertion =
        !guest.nix.settings.sandbox-fallback && guest.nix.settings.sync-before-registering;
      message = "the guest nix daemon should keep the build-box hardening settings";
    }
    {
      assertion = lib.elem "ca-derivations" guest.nix.settings.extra-experimental-features;
      message = "the guest nix daemon should enable every advertised experimental system feature";
    }
  ];

  failures = map (a: a.message) (lib.filter (a: !a.assertion) assertions);
in
  assert lib.assertMsg (failures == []) (
    "builder-vm:\n  " + lib.concatStringsSep "\n  " failures
  );
    pkgs.runCommand "ix-test-builder-vm" {
      __structuredAttrs = true;
      # Building the scripts runs their bash -n + shellcheck check phases.
      # They are aarch64-darwin derivations (see darwinPkgs above), so only
      # the darwin leg of CI can realize them; the linux leg stays eval-only.
      scripts = lib.optionals (pkgs.stdenv.hostPlatform.system == "aarch64-darwin") (
        builtins.attrValues withGuest.services.builder-vm.packages
        ++ [withGuest.services.builder-vm.packages.vm-install.tests.grow-only]
      );
    } ''
      ${lib.optionalString (pkgs.stdenv.hostPlatform.system == "aarch64-darwin") ''
        export VM_DEPLOY_ARGS="$TMPDIR/vm-deploy-args"
        ${lib.getExe withGuest.services.builder-vm.packages.vm-deploy}
        printf '%s\n' \
          switch \
          --flake \
          ${lib.escapeShellArg deployFlake} \
          --target-host \
          root@vm \
          > "$TMPDIR/vm-deploy-expected"
        cmp "$TMPDIR/vm-deploy-expected" "$VM_DEPLOY_ARGS"
      ''}
      mkdir -p "$out"
    ''
