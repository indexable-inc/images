# System-level (nix-darwin) adapter for declarative-but-writable files: the
# /etc counterpart of modules/home/mutable-files.nix. Same model, run as root
# at activation — a declared file is deployed as a plain regular file (no
# read-only store symlink), seeded from its Nix-declared base, with local
# edits tracked as logical, format-aware diffs by `index-delta`
# (packages/index-delta).
#
# Persistence per file: `ephemeral` (default) resets from the base at every
# activation and boot after archiving the drift's diff to the journal;
# `durable` keeps edits and stages base changes under drift as conflicts in
# `index-delta status` instead of touching the file.
#
# State lives at /var/db/index-delta (the macOS convention for system state),
# so `sudo index-delta --state-dir /var/db/index-delta status` inspects the
# system queue without clashing with any per-user state.
{indexPackages}: {
  config,
  lib,
  pkgs,
  ...
}: let
  inherit
    (lib)
    mkOption
    mkIf
    types
    ;

  cfg = config.mutable;

  stateDir = "/var/db/index-delta";

  defaultPackage = (indexPackages pkgs.stdenv.hostPlatform.system).index-delta;

  fileType = types.submodule ({name, ...}: {
    options = {
      source = mkOption {
        type = types.nullOr types.path;
        default = null;
        description = "File holding the declared content. Exactly one of `source` and `text` must be set.";
      };

      text = mkOption {
        type = types.nullOr types.lines;
        default = null;
        description = "Inline declared content. Exactly one of `source` and `text` must be set.";
      };

      format = mkOption {
        type = types.nullOr (types.enum [
          "json"
          "toml"
          "yaml"
          "plist"
          "keyvalue"
          "text"
        ]);
        default = null;
        description = "Logical-diff format; null auto-detects from extension then content and sticks.";
      };

      persistence = mkOption {
        type = types.enum [
          "ephemeral"
          "durable"
        ];
        default = "ephemeral";
        description = "`ephemeral`: reset from base each activation/boot (drift journaled). `durable`: edits survive; base changes under drift queue as conflicts.";
      };

      sourceFile = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Repo path of the declared content, so `index-delta status` can point `apply-ops` at it. Informational.";
      };

      declaredAt = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Nix file (and line) that declared this entry. Informational.";
      };

      _target = mkOption {
        type = types.str;
        internal = true;
        default =
          if lib.hasPrefix "/" name
          then name
          else "/${name}";
        description = "Resolved absolute target path.";
      };
    };
  });

  files = lib.attrValues cfg.files;

  contentFor = file:
    if file.text != null
    then pkgs.writeText (baseNameOf file._target) file.text
    else file.source;

  manifest = (pkgs.formats.json {}).generate "index-delta-system-manifest.json" {
    files =
      map (
        file:
          {
            path = file._target;
            source = "${contentFor file}";
            inherit (file) persistence;
          }
          // lib.optionalAttrs (file.format != null) {inherit (file) format;}
          // lib.optionalAttrs (file.sourceFile != null) {inherit (file) sourceFile;}
          // lib.optionalAttrs (file.declaredAt != null) {inherit (file) declaredAt;}
      )
      files;
  };
in {
  options.mutable = {
    package = mkOption {
      type = types.package;
      default = defaultPackage;
      defaultText = lib.literalExpression "index.packages.\${system}.index-delta";
      description = "The `index-delta` package that seeds and tracks the files.";
    };

    files = mkOption {
      type = types.attrsOf fileType;
      default = {};
      example = lib.literalExpression ''
        {
          "etc/pf.anchors/dev".text = "...";
        }
      '';
      description = ''
        System files declared in Nix but deployed as plain writable files,
        seeded and tracked by `index-delta` as root. Attribute names are
        absolute paths (a leading `/` is implied).
      '';
    };
  };

  config = mkIf (cfg.files != {}) {
    assertions =
      map (file: {
        assertion = (file.source == null) != (file.text == null);
        message = "mutable.files.\"${file._target}\": set exactly one of `source` and `text`.";
      })
      files;

    system.activationScripts.postActivation.text = lib.mkAfter ''
      echo "seeding mutable files (index-delta)..."
      ${cfg.package}/bin/index-delta --state-dir ${stateDir} activate --manifest ${manifest}
    '';

    # Boot-time reseed: ephemeral system files start every boot from their
    # declared base. `reseed` works off snapshotted bases, so no manifest.
    launchd.daemons.index-delta-reseed = {
      serviceConfig = {
        Label = "org.nixos.index-delta-reseed";
        ProgramArguments = [
          "${cfg.package}/bin/index-delta"
          "--state-dir"
          stateDir
          "reseed"
        ];
        RunAtLoad = true;
      };
    };
  };
}
