# Declarative Raycast Focus session defaults (macOS).
#
# Raycast keeps Focus settings in the `com.raycast.macos` preferences domain.
# The title and filter mode are plain strings, but the session duration is stored
# as a plist <data> value whose bytes are a JSON document. nix-darwin's
# attrset->plist generator has no <data> type, so it cannot express that key;
# this module writes every managed key with `defaults` instead, encoding the
# duration as the JSON-in-data Raycast expects (hex via /usr/bin/od, which always
# ships on macOS, so the module pulls in no extra packages).
#
# Caveat: Raycast is a long-running app and rewrites these keys when you edit a
# Focus session in its UI. Values here are applied at `home-manager switch` time;
# a running Raycast may not pick them up until it is relaunched. Quit Raycast
# before switching if you need a hard guarantee.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.programs.raycast.focus;

  durationJson = builtins.toJSON {
    inherit (cfg.duration) id title;
    duration = cfg.duration.seconds;
  };

  domain = "com.raycast.macos";

  # key -> string value (written as a plist string)
  stringKeys = {
    "raycast-startFocusSession-title" = cfg.title;
    "raycast-startFocusSession-filter-mode" = cfg.filterMode;
  };

  # key -> JSON document Raycast stores as a plist <data> value
  dataKeys = {
    "raycast-startFocusSession-duration" = durationJson;
  }
  // lib.optionalAttrs (cfg.categoryBlockableItems != null) {
    "raycast-focus-category-blockable-items" = cfg.categoryBlockableItems;
  }
  // lib.optionalAttrs (cfg.blockableItems != null) {
    "raycast-startFocusSession-blockable-items" = cfg.blockableItems;
  };

  writeString =
    key: val:
    "$DRY_RUN_CMD /usr/bin/defaults write ${domain} ${lib.escapeShellArg key} ${lib.escapeShellArg val}";

  # `defaults write -data` takes contiguous hex; od emits the JSON bytes as hex.
  writeData =
    key: json:
    "$DRY_RUN_CMD /usr/bin/defaults write ${domain} ${lib.escapeShellArg key} -data \"$(printf '%s' ${lib.escapeShellArg json} | /usr/bin/od -An -v -tx1 | /usr/bin/tr -d ' \\n')\"";
in
{
  options.programs.raycast.focus = {
    enable = lib.mkEnableOption "declarative Raycast Focus session defaults (macOS)";

    title = lib.mkOption {
      type = lib.types.str;
      default = "Deep Work";
      example = "Deep Work";
      description = "Title shown for the Focus session.";
    };

    filterMode = lib.mkOption {
      type = lib.types.enum [
        "block"
        "allow"
      ];
      default = "block";
      description = "Whether the blockable list is a blocklist (`block`) or an allowlist (`allow`).";
    };

    duration = {
      seconds = lib.mkOption {
        type = lib.types.ints.positive;
        default = 900;
        example = 1800;
        description = "Default Focus session length, in seconds.";
      };
      title = lib.mkOption {
        type = lib.types.str;
        default = "15 minutes";
        description = "Human label Raycast shows for the duration preset.";
      };
      id = lib.mkOption {
        type = lib.types.str;
        default = "F0D14788-CE57-43BE-9CFE-1D1C6FE30BA8";
        description = "Opaque identifier Raycast assigns to the duration preset.";
      };
    };

    categoryBlockableItems = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = ''
        Raw JSON for `raycast-focus-category-blockable-items` (the blockable
        apps/sites list). Unmanaged (null) by default: the value is a large
        Raycast-internal blob with embedded color/icon archives, so capture it
        from your own plist if you want to pin it.
      '';
    };

    blockableItems = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "Raw JSON for `raycast-startFocusSession-blockable-items`. See `categoryBlockableItems`.";
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = pkgs.stdenv.hostPlatform.isDarwin;
        message = "programs.raycast.focus is macOS-only (it writes the com.raycast.macos defaults domain).";
      }
    ];

    home.activation.raycastFocus = config.lib.dag.entryAfter [ "writeBoundary" ] (
      lib.concatStringsSep "\n" (
        [ "$VERBOSE_ECHO 'configuring Raycast Focus defaults'" ]
        ++ lib.mapAttrsToList writeString stringKeys
        ++ lib.mapAttrsToList writeData dataKeys
      )
    );
  };
}
