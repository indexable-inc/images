# Per-generation eval-provenance manifest for home-manager (#2415).
#
# A deployed config file (a `~/.config` symlink, a launchd plist) gives no
# way back to the nix expression that generated it; xattrs cannot carry the
# backlink (NAR has no xattr field, and Linux forbids `user.*` xattrs on the
# symlinks home-manager deploys). This module bakes the backlink into the
# generation itself: `lib/provenance.nix` walks the evaluated configuration
# (`home.file`, `xdg.configFile`, `launchd.agents`, plus linked
# `programs.*.settings` sites) and the rendered manifest is linked into the
# activation package, so the live profile always carries
# `~/.local/state/nix/profiles/home-manager/provenance.json` for its own
# generation and old generations keep theirs. Query time is a JSON read
# (`whence <path>`), zero eval.
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
        entries = provenance.homeCollectors {inherit options config;};
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

  # `home.extraBuilderCommands` (not `home.file`): the walker reads
  # `home.file`, so materializing the manifest through it would make the
  # manifest an input of itself.
  config = lib.mkIf cfg.enable {
    home.extraBuilderCommands = ''
      ln -s ${cfg.manifest} $out/provenance.json
    '';
  };
}
