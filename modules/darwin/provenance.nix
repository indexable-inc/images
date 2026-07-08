# Per-generation eval-provenance manifest for nix-darwin (#2415): the
# darwin counterpart of modules/home/provenance.nix, covering the system
# surfaces (`environment.etc`, `launchd.agents`/`daemons`). The rendered
# manifest is linked into the system derivation, so
# `/run/current-system/provenance.json` always describes the running
# generation and old generations keep theirs; `whence </etc/...>` reads it
# with zero eval.
# Callers inject the walker (`ix.provenance`, lib/provenance.nix) so this
# file stays importable as a bare module argument set; see the flake's
# homeModules/darwinModules wiring.
{provenance}: {
  config,
  options,
  lib,
  pkgs,
  ...
}: let
  cfg = config.provenance;
in {
  options.provenance = {
    enable = lib.mkEnableOption "baking a provenance manifest (deployed path -> defining nix file:line) into each generation";

    rev = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = lib.literalExpression "self.rev or self.dirtyRev or null";
      description = ''
        Configuration flake revision recorded on every manifest entry, so
        `whence` can print which checkout defined a file. Pass
        `self.rev or self.dirtyRev or null` from the consuming flake.
      '';
    };

    entries = lib.mkOption {
      type = lib.types.raw;
      internal = true;
      readOnly = true;
      default = provenance.manifestFor {
        inherit options;
        inherit (cfg) rev;
        entries = provenance.darwinCollectors {inherit options config;};
      };
      description = "Rendered manifest attrset (deployed path -> provenance).";
    };

    manifest = lib.mkOption {
      type = lib.types.package;
      internal = true;
      readOnly = true;
      default = (pkgs.formats.json {}).generate "provenance.json" cfg.entries;
      description = "The provenance.json for this generation.";
    };
  };

  # `system.systemBuilderCommands` (not `environment.etc`): the walker reads
  # `environment.etc`, so materializing the manifest through it would make
  # the manifest an input of itself.
  config = lib.mkIf cfg.enable {
    system.systemBuilderCommands = ''
      ln -s ${cfg.manifest} $out/provenance.json
    '';
  };
}
