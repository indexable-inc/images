# Ghostty terminal configuration, generated entirely from Nix.
#
# Single source of truth: the `settings` attrset below. It is rendered in-store
# by pkgs.formats.keyValue (the same generator home-manager's programs.ghostty
# uses) and written to the macOS config path with an in-store source, so there
# is no out-of-store symlink back into the repo. Edit Nix, switch, done.
#
# Repeated keybind/shader families (repo `cd` shortcuts, directional split
# focus, shader stack) are built algebraically by mapping over data, so adding
# one is a one-line data edit rather than a hand-copied config line.
#
# Themes (custom-dark/custom-light) and shaders live in ghostty/{themes,shaders}
# and are linked into ~/.config/ghostty separately in profiles/workstation.nix; this file
# only references them by name / absolute path.
{
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.users.andrewgazelka;
  home = config.home.homeDirectory;
  projects = "${home}/Projects/indexable-inc";
  shaderDir = "${home}/.config/ghostty/shaders";

  # cmd+<n> clears the half-typed line (Ctrl-U = \x15) then types `cd <repo>\r`.
  # Native Ghostty text injection, so no Accessibility/System-Events permission
  # and no leaking keys to other apps. MUST bind the physical `digit_N`
  # position: since Ghostty 1.2.0 the default `cmd+digit_N=goto_tab:N` (physical)
  # wins over a logical `cmd+1`, so binding `cmd+digit_N` is what overrides the
  # tab switch. AeroSpace must NOT bind cmd-1/2/3 or its global tap swallows them
  # before Ghostty sees the keys (see profiles/workstation.nix).
  repoShortcuts = lib.mapAttrsToList (n: path: ''cmd+digit_${n}=text:\x15cd ${path}\r'') {
    "1" = "${projects}/ix";
    "2" = "${projects}/index";
    "3" = cfg.paths.indexCheckout;
  };

  # Directional split focus: same shape for all four arrows.
  splitNav = map (d: "cmd+shift+${d}=goto_split:${d}") [
    "left"
    "right"
    "up"
    "down"
  ];

  # Shader stack, applied in order.
  shaders = map (s: "${shaderDir}/${s}") [
    "crt.glsl" # subtle CRT look
    "focus-border.glsl" # static dim-white outline on the focused pane (gated by iFocus)
  ];

  settings = {
    font-size = 10;
    font-family = "Berkeley Mono";
    theme = "dark:custom-dark,light:custom-light";

    split-divider-color = "2a2a2a";
    scrollback-limit = 4294967295;
    scrollbar = "never";
    link-previews = true;

    clipboard-paste-protection = false;
    clipboard-read = "allow";

    macos-icon = "custom-style";
    macos-icon-ghost-color = "111111";
    macos-icon-screen-color = "222222";
    macos-icon-frame = "chrome";
    macos-titlebar-proxy-icon = "hidden";
    macos-option-as-alt = true;
    macos-window-buttons = "hidden";

    window-padding-x = 5;
    window-padding-y = 5;
    window-decoration = "none";

    cursor-style = "block";
    cursor-style-blink = false;

    background-blur = 20;
    background-opacity = "1.0";
    background-opacity-cells = true;

    # Track Ghostty nightlies. The declared Homebrew cask is `ghostty@tip`, so
    # keep the in-app Sparkle updater OFF but pinned to the matching `tip`
    # channel so any reload reports the right track. Updates come from the cask
    # (`brew upgrade --cask ghostty@tip`), not the in-app updater.
    auto-update = "off";
    auto-update-channel = "tip";

    confirm-close-surface = false;

    custom-shader = shaders;
    custom-shader-animation = true;

    keybind =
      [
        # Send Ctrl+U for Cmd+Backspace (delete to start of line).
        ''cmd+backspace=text:\x15''

        "cmd+n=new_window"
        "cmd+t=new_tab"
        "cmd+shift+]=next_tab"
        "cmd+shift+[=previous_tab"
        "cmd+w=close_surface"

        # Copy entire scrollback: select all, then copy.
        "cmd+shift+c=select_all"
        "chain=copy_to_clipboard"
        "cmd+shift+v=paste_from_clipboard"

        "cmd+0=reset_font_size"
        "cmd+plus=increase_font_size:1"
        "cmd+minus=decrease_font_size:1"
        "cmd+shift+r=reload_config"

        # Sequential split focus.
        "cmd+]=goto_split:next"
        "cmd+[=goto_split:previous"
        "alt+tab=goto_split:next"
        "shift+alt+tab=goto_split:previous"

        # Copy file path/URL to clipboard.
        "performable:cmd+shift+u=copy_url_to_clipboard"

        # Scrollback navigation.
        "cmd+up=scroll_to_top"
        "cmd+down=scroll_to_bottom"

        ''shift+enter=text:\x1b\r''
        ''cmd+enter=text:\x1b[13;9u''
        "cmd+a=unbind"
        "cmd+shift+a=toggle_command_palette"
        "cmd+shift+s=write_screen_file:copy"
      ]
      ++ repoShortcuts
      ++ splitNav;
  };

  configFile =
    (pkgs.formats.keyValue {
      listsAsDuplicateKeys = true;
      mkKeyValue = lib.generators.mkKeyValueDefault {} " = ";
    }).generate
    "ghostty-config"
    settings;
in {
  # In-store source (no mkOutOfStoreSymlink). Written to the macOS
  # Application Support path Ghostty already loads; shaders/themes stay under
  # ~/.config/ghostty (linked in profiles/workstation.nix). Only one config path is
  # written so cumulative settings (e.g. custom-shader) are not double-applied.
  home.file."Library/Application Support/com.mitchellh.ghostty/config" = {
    source = configFile;
    force = true;
  };
}
