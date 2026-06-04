# Declarative Raycast Focus session defaults (macOS).
#
# Raycast keeps Focus settings in the `com.raycast.macos` preferences domain.
# The title and filter mode are plain strings, but the session duration and the
# blocklist are stored as plist <data> values whose bytes are JSON documents.
# nix-darwin's attrset->plist generator has no <data> type, so it cannot express
# those keys; this module writes every managed key with `defaults` instead.
#
# The blocklist is the interesting part. Raycast stores it as a JSON array of
# richly-decorated items, each carrying an NSKeyedArchiver color archive and an
# icon descriptor. Rather than make you paste those ~170KB blobs, declare a clean
# list of apps and websites (`focus.block`) and the module synthesizes the exact
# JSON Raycast expects: app icons resolve from the `.app` path, website favicons
# from Raycast's favicon API, and a single shared theme-dynamic color archive
# (the committed `raycast-item-color.b64`) tints every row. This was
# reverse-engineered from a live plist and verified end to end: Raycast accepts
# and blocks items synthesized this way. See `raycast-focus-items.py`.
#
# Note: custom Focus *categories* (the named groups in Raycast's UI) live in
# Raycast's separate encrypted SQLite store, not this defaults domain, so they
# cannot be managed here. This module manages the session's flat blocklist, which
# is what actually gets blocked.
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

  domain = "com.raycast.macos";

  durationJson = builtins.toJSON {
    inherit (cfg.duration) id title;
    duration = cfg.duration.seconds;
  };

  blockActive = cfg.block.apps != [ ] || cfg.block.websites != [ ];

  # Build the blockable-items JSON at build time (python is a build-only dep, not
  # in the activation closure); the activation script just reads this static file.
  focusInput = pkgs.writeText "raycast-focus-input.json" (
    builtins.toJSON {
      apps = map (a: { inherit (a) bundleId name path; }) cfg.block.apps;
      inherit (cfg.block) websites;
    }
  );
  focusItems = pkgs.runCommand "raycast-focus-items.json" {
    nativeBuildInputs = [ pkgs.python3 ];
  } "python3 ${./raycast-focus-items.py} ${focusInput} ${./raycast-item-color.b64} > $out";

  # Each blockable key resolves to a raw JSON string (escape hatch), the
  # generated file, or nothing.
  selectionSource =
    if cfg.blockableItems != null then
      { raw = cfg.blockableItems; }
    else if blockActive then
      { file = focusItems; }
    else
      null;
  categorySource =
    if cfg.categoryBlockableItems != null then
      { raw = cfg.categoryBlockableItems; }
    else if blockActive then
      { file = focusItems; }
    else
      null;

  writeString =
    key: val:
    "$DRY_RUN_CMD /usr/bin/defaults write ${domain} ${lib.escapeShellArg key} ${lib.escapeShellArg val}";

  # `defaults write -data` takes contiguous hex; od emits the JSON bytes as hex.
  writeData =
    key: json:
    "$DRY_RUN_CMD /usr/bin/defaults write ${domain} ${lib.escapeShellArg key} -data \"$(printf '%s' ${lib.escapeShellArg json} | /usr/bin/od -An -v -tx1 | /usr/bin/tr -d ' \\n')\"";
  writeDataFile =
    key: file:
    "$DRY_RUN_CMD /usr/bin/defaults write ${domain} ${lib.escapeShellArg key} -data \"$(/usr/bin/od -An -v -tx1 ${file} | /usr/bin/tr -d ' \\n')\"";
  writeSource = key: src: if src ? raw then writeData key src.raw else writeDataFile key src.file;

  appSubmodule = lib.types.submodule {
    options = {
      bundleId = lib.mkOption {
        type = lib.types.str;
        example = "com.tinyspeck.slackmacgap";
        description = "Application bundle identifier (CFBundleIdentifier).";
      };
      name = lib.mkOption {
        type = lib.types.str;
        example = "Slack";
        description = "Display name shown in the blocklist.";
      };
      path = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "/System/Applications/Maps.app";
        description = ''
          Absolute path to the `.app` bundle (used to render the icon). Defaults
          to `/Applications/<name>.app`; set it for system apps
          (`/System/Applications/...`) or non-standard install locations.
        '';
      };
    };
  };
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

    block = {
      apps = lib.mkOption {
        type = lib.types.listOf appSubmodule;
        default = [ ];
        example = lib.literalExpression ''
          [
            { bundleId = "com.tinyspeck.slackmacgap"; name = "Slack"; }
            { bundleId = "com.apple.Maps"; name = "Maps"; path = "/System/Applications/Maps.app"; }
          ]
        '';
        description = ''
          Applications to put on the Focus blocklist. The module synthesizes the
          icon and styling Raycast expects from each entry, so you only declare
          the bundle id, name, and (optionally) path.
        '';
      };
      websites = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [ ];
        example = [
          "x.com"
          "youtube.com"
        ];
        description = "Website domains to put on the Focus blocklist (favicons resolve automatically).";
      };
    };

    categoryBlockableItems = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = ''
        Escape hatch: raw JSON for `raycast-focus-category-blockable-items`. When
        set, overrides what `block` would generate for this key. Capture it from
        your own plist if you need a value `block` cannot express.
      '';
    };

    blockableItems = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "Escape hatch: raw JSON for `raycast-startFocusSession-blockable-items`. See `categoryBlockableItems`.";
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
        ++ [
          (writeString "raycast-startFocusSession-title" cfg.title)
          (writeString "raycast-startFocusSession-filter-mode" cfg.filterMode)
          (writeData "raycast-startFocusSession-duration" durationJson)
        ]
        ++ lib.optional (selectionSource != null) (
          writeSource "raycast-startFocusSession-blockable-items" selectionSource
        )
        ++ lib.optional (categorySource != null) (
          writeSource "raycast-focus-category-blockable-items" categorySource
        )
      )
    );
  };
}
