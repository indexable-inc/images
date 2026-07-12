# Declarative-but-writable files: `mutable.files.<path>` declares content in
# Nix, but the deployed file is a plain regular file the owning app can edit
# freely — no read-only store symlink. `index-delta` (packages/index-delta)
# seeds each file from its declared base at activation and tracks local edits
# as *logical*, format-aware diffs (JSON diffed as JSON, TOML as TOML, ...).
#
# There is deliberately no auto-merge. Persistence is per file:
#
#   * `ephemeral` (the default): edits are scratch. Every activation and every
#     login rewrites the file from its base; the drift's logical diff is
#     archived to `index-delta journal` first, so nothing is silently lost.
#   * `durable`: edits survive. When the declared base changes under local
#     drift, the incoming base is parked as *staged* and the file queues in
#     `index-delta status` for resolution (discard / adopt / absorb via
#     `apply-ops` / snooze) — the file itself is never touched.
#
# The resolution queue (`index-delta status --json`) is designed for a model
# to read: both diffs (yours + incoming) as addressed ops, plus the overlap
# between them. `index-delta apply-ops <repo-file> <ops.json>` replays chosen
# ops onto the Nix source, so drift can be absorbed into the repo instead of
# resolved by hand.
#
# Closed over the per-system flake package set (for the `index-delta` binary)
# and the portable-services home module (for the login reseed agent: native
# launchd on macOS, systemd user unit on Linux). See flake.nix homeModules.
{
  indexPackages,
  portableServicesModule,
}: {
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

  defaultPackage = (indexPackages pkgs.stdenv.hostPlatform.system).index-delta;

  # Attribute names are target paths: absolute and `~/`-prefixed pass
  # through; anything else is relative to home.
  targetPath = name:
    if lib.hasPrefix "/" name || lib.hasPrefix "~" name
    then name
    else "~/${name}";

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
        description = ''
          Logical-diff format. Null (the default) auto-detects from the
          target's extension, then the base's content; the detected format is
          recorded in the file's state so it can never flip under an existing
          diff. Set it only to override detection (e.g. an extensionless
          ghostty config that should diff as `keyvalue`).
        '';
      };

      persistence = mkOption {
        type = types.enum [
          "ephemeral"
          "durable"
        ];
        default = "ephemeral";
        description = ''
          `ephemeral` (default): edits are scratch — reset from the base at
          every activation and login, with the drift's logical diff archived
          to the journal first. `durable`: edits survive; a base change under
          drift stages a conflict in `index-delta status` instead of touching
          the file.
        '';
      };

      sourceFile = mkOption {
        type = types.nullOr types.str;
        default = null;
        example = "modules/profiles/ghostty/config";
        description = ''
          Repo path of the file holding the declared content, recorded in the
          file's state so `index-delta status` can point "absorb this drift
          into Nix" edits (`index-delta apply-ops`) at the right file. Purely
          informational — omitting it only means status cannot suggest where
          to apply ops.
        '';
      };

      declaredAt = mkOption {
        type = types.nullOr types.str;
        default = null;
        example = "users/alice/home.nix:42";
        description = "Nix file (and line) that declared this entry, surfaced by `index-delta status`. Informational.";
      };

      _target = mkOption {
        type = types.str;
        internal = true;
        default = targetPath name;
        description = "Resolved target path handed to the manifest.";
      };
    };
  });

  files = lib.attrValues cfg.files;

  contentFor = file:
    if file.text != null
    then pkgs.writeText (baseNameOf file._target) file.text
    else file.source;

  manifest = (pkgs.formats.json {}).generate "index-delta-manifest.json" {
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
  imports = [portableServicesModule];

  options.mutable = {
    package = mkOption {
      type = types.package;
      default = defaultPackage;
      defaultText = lib.literalExpression "index.packages.\${system}.index-delta";
      description = "The `index-delta` package that seeds and tracks the files.";
    };

    loginReseed = mkOption {
      type = types.bool;
      default = true;
      description = ''
        Run `index-delta reseed` at login so ephemeral files start every
        session from their declared base (drift is archived to the journal
        first). Durable files are never touched by reseed.
      '';
    };

    files = mkOption {
      type = types.attrsOf fileType;
      default = {};
      example = lib.literalExpression ''
        {
          ".config/ghostty/config" = {
            source = ./ghostty-config;
            format = "keyvalue";
          };
          ".config/lazygit/config.yml" = {
            text = "gui:\n  theme: dark\n";
            persistence = "durable";
          };
        }
      '';
      description = ''
        Files declared in Nix but deployed as plain writable files, seeded and
        tracked by `index-delta`. Attribute names are target paths (relative
        to home unless absolute or `~/`-prefixed).
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

    # After writeBoundary so activation ordering matches home.file targets;
    # `activate` seeds new files, reseeds ephemeral ones, and gates durable
    # ones (staging conflicts rather than touching drifted files), then prints
    # one line per file.
    home.activation.mutableFiles = config.lib.dag.entryAfter ["writeBoundary"] ''
      $DRY_RUN_CMD ${cfg.package}/bin/index-delta activate --manifest ${manifest}
    '';

    # Login reseed: ephemeral files reset every session, not just at switch
    # time. `reseed` works off the snapshotted bases, so it needs no manifest
    # and never blocks on the repo being checked out.
    services.portable.index-delta-reseed = {
      enable = cfg.loginReseed;
      description = "Reset ephemeral mutable files to their declared base";
      command = [
        "${cfg.package}/bin/index-delta"
        "reseed"
      ];
      runAtLoad = true;
    };
  };
}
