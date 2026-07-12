# Cross-platform Nix daemon defaults and flake registry pins. Everything here
# touches only option paths nix-darwin and NixOS declare identically
# (nix.settings, nix.gc.automatic, nix.registry), so one module serves both
# platforms: import it and set `nix.daemonDefaults.enable = true` and/or
# `nix.registryPins`. Host-specific knobs (substituters, caches, job limits,
# sandbox, nix-path) stay in each host file.
{
  config,
  lib,
  ...
}: let
  inherit
    (lib)
    mkDefault
    mkEnableOption
    mkIf
    mkOption
    types
    ;

  cfg = config.nix;

  registryReference = input:
    {
      type = "path";
      path = input.outPath;
    }
    // lib.filterAttrs (
      name: _:
        lib.elem name [
          "lastModified"
          "narHash"
          "rev"
        ]
    )
    input;
in {
  options.nix = {
    daemonDefaults.enable = mkEnableOption "shared Nix daemon defaults: experimental features, keep-* retention, automatic GC";

    registryPins = mkOption {
      type = types.attrsOf types.raw;
      default = {};
      example = lib.literalExpression "{ inherit (inputs) nixpkgs index; }";
      description = ''
        Flake registry aliases: point each name at the exact flake input the
        consuming config is built from, so `nix shell nixpkgs#x` and
        `nix run index#foo` resolve fleet-wide against the pinned revs (bump
        with `nix flake update`). Values are the consumer's own flake inputs;
        nothing is pinned by default.
      '';
    };
  };

  config.nix = {
    registry = lib.mapAttrs (_: input: {to = registryReference input;}) cfg.registryPins;

    # Nix daemon settings shared verbatim by both platforms.
    settings = mkIf cfg.daemonDefaults.enable {
      experimental-features = [
        "nix-command"
        "flakes"
        "ca-derivations"
        "dynamic-derivations"
        "recursive-nix"
        "impure-derivations"
        "blake3-hashes"
      ];
      # Silence the per-evaluation "Git tree '...' is dirty" warning: this
      # config is deployed from a perpetually dirty tree, so it is pure noise.
      warn-dirty = false;
      # auto-optimise-store is per-host (a workstation may disable it for fast
      # dirty-tree adds; a server keeps it on), so it stays in each host file.
      keep-derivations = true;
      keep-outputs = true;
      connect-timeout = 5;
    };

    # mkDefault so a host opts out with a plain `nix.gc.automatic = false`.
    gc.automatic = mkIf cfg.daemonDefaults.enable (mkDefault true);
  };
}
