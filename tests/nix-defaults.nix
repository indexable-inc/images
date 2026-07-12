# Eval-only checks for modules/nix/defaults.nix: the module must stay inert
# until enabled, apply the shared daemon settings exactly, let hosts override
# gc.automatic, and map registryPins onto nix.registry.<name>.flake. Stubbed
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
          options.flake = lib.mkOption {
            type = lib.types.nullOr lib.types.raw;
            default = null;
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
    rev = "0000000000000000000000000000000000000000";
  };
  pinned = evalWith {nix.registryPins.example = fakeInput;};

  expectedSettings = {
    experimental-features = [
      "nix-command"
      "flakes"
      "ca-derivations"
      "dynamic-derivations"
      "recursive-nix"
      "impure-derivations"
      "blake3-hashes"
    ];
    warn-dirty = false;
    keep-derivations = true;
    keep-outputs = true;
    connect-timeout = 5;
  };

  # Real NixOS eval: proves the module composes with the actual nix.* options.
  nixosEval =
    (nixpkgs.lib.nixosSystem {
      modules = [
        (paths.root + "/modules/nix/defaults.nix")
        {
          nixpkgs.hostPlatform = "x86_64-linux";
          nix.daemonDefaults.enable = true;
          nix.registryPins.nixpkgs = nixpkgs;
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
      assertion = enabled.nix.settings == expectedSettings;
      message = "enable must apply the shared daemon settings exactly";
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
      assertion = pinned.nix.registry.example.flake.outPath == fakeInput.outPath;
      message = "registryPins.<name> must land on nix.registry.<name>.flake";
    }
    {
      assertion = pinned.nix.settings == {} && pinned.nix.gc.automatic == false;
      message = "registryPins alone must not enable the daemon defaults";
    }
    {
      assertion = nixosEval.nix.settings.warn-dirty == false;
      message = "NixOS eval must carry warn-dirty = false";
    }
    {
      assertion = lib.elem "ca-derivations" nixosEval.nix.settings.experimental-features;
      message = "NixOS eval must carry the experimental features";
    }
    {
      assertion = nixosEval.nix.gc.automatic == true;
      message = "NixOS eval must default gc.automatic on";
    }
    {
      assertion = nixosEval.nix.registry.nixpkgs.flake.outPath == nixpkgs.outPath;
      message = "NixOS eval must pin nix.registry.nixpkgs to the given input";
    }
  ];

  failures = map (a: a.message) (lib.filter (a: !a.assertion) assertions);
in
  assert lib.assertMsg (failures == []) ("nix-defaults test failures:\n" + lib.concatStringsSep "\n" failures);
    pkgs.runCommand "ix-test-nix-defaults" {} ''
      mkdir -p "$out"
    ''
