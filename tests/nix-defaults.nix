# Eval-only checks for modules/nix/defaults.nix: the module must stay inert
# until enabled, apply the shared daemon settings exactly, let hosts override
# gc.automatic, and map registryPins onto nix.registry.<name>.to. Stubbed
# option shapes stand in for the nix-darwin/NixOS declarations (index has no
# nix-darwin input); one real nixosSystem eval covers the NixOS side.
{
  lib,
  pkgs,
  paths,
  nixpkgs,
}: let
  # Option stubs mirroring the shape both nix-darwin and NixOS declare for the
  # paths the module touches.
  stubs = {
    options.nix = {
      settings = lib.mkOption {
        type = lib.types.attrsOf lib.types.raw;
        default = {};
      };
      gc.automatic = lib.mkOption {
        type = lib.types.bool;
        default = false;
      };
      registry = lib.mkOption {
        type = lib.types.attrsOf (lib.types.submodule {
          options.to = lib.mkOption {
            type = lib.types.attrsOf lib.types.raw;
          };
        });
        default = {};
      };
    };
  };

  evalWith = extra:
    (lib.evalModules {
      modules = [stubs (paths.root + "/modules/nix/defaults.nix") extra];
    }).config;

  inert = evalWith {};
  enabled = evalWith {nix.daemonDefaults.enable = true;};
  hostOverride = evalWith {
    nix.daemonDefaults.enable = true;
    nix.gc.automatic = false;
  };

  fakeInput = {
    outPath = "/fake/input";
    lastModified = 1;
  };
  pinned = evalWith {nix.registryPins.example = fakeInput;};

  # Real NixOS eval: proves the module composes with the actual nix.* options.
  nixosEval =
    (nixpkgs.lib.nixosSystem {
      modules = [
        (paths.root + "/modules/nix/defaults.nix")
        {
          nixpkgs.hostPlatform = "x86_64-linux";
          nix.daemonDefaults.enable = true;
          nix.registryPins.nixpkgs = fakeInput;
          system.stateVersion = "25.05";
        }
      ];
    }).config;

  assertions = [
    {
      assertion = inert.nix.settings == {} && inert.nix.gc.automatic == false && inert.nix.registry == {};
      message = "module must be inert with defaults";
    }
    {
      assertion = enabled.nix.gc.automatic == true;
      message = "enable must default gc.automatic on";
    }
    {
      assertion = hostOverride.nix.gc.automatic == false;
      message = "host `nix.gc.automatic = false` must beat the module's mkDefault";
    }
    {
      assertion =
        pinned.nix.registry.example.to
        == {
          type = "path";
          path = fakeInput.outPath;
          inherit (fakeInput) lastModified;
        };
      message = "registryPins.<name> must render a pinned path registry reference";
    }
    {
      assertion = pinned.nix.settings == {} && pinned.nix.gc.automatic == false;
      message = "registryPins alone must not enable the daemon defaults";
    }
    {
      assertion = lib.getAttrs (lib.attrNames enabled.nix.settings) nixosEval.nix.settings == enabled.nix.settings;
      message = "NixOS must preserve the shared settings produced by the platform-neutral module";
    }
    {
      assertion = nixosEval.nix.gc.automatic == true;
      message = "NixOS eval must default gc.automatic on";
    }
    {
      assertion = nixosEval.nix.registry.nixpkgs.to.path == fakeInput.outPath;
      message = "registryPins.nixpkgs must override NixOS's default registry pin";
    }
  ];

  failures = map (a: a.message) (lib.filter (a: !a.assertion) assertions);
in
  assert lib.assertMsg (failures == []) ("nix-defaults test failures:\n" + lib.concatStringsSep "\n" failures);
    pkgs.runCommand "ix-test-nix-defaults" {__structuredAttrs = true;} ''
      mkdir -p "$out"
    ''
