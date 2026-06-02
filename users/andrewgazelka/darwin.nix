# Personal-but-shareable nix-darwin module for github:andrewgazelka: the
# Homebrew package set (GUI casks, the `mas` brew, and Mac App Store apps).
#
# Hoisted out of the private ~/.config/nix so the list lives in the open
# monorepo alongside the rest of the user's workstation glue (home.nix). It is
# the companion to homeModules.andrewgazelka: that one owns the home-manager
# services, this one owns the system-level Homebrew packages.
#
# Importing this module is the opt-in: it only contributes the package lists
# (`homebrew.casks`/`brews`/`masApps`, which nix-darwin merges across modules).
# The consuming host keeps the policy knobs it owns: `homebrew.enable`,
# `onActivation.cleanup`, and any taps. Because the lists merge, the consumer
# can still add host-specific casks of its own without re-declaring these.
#
# These are GUI apps and Mac-App-Store apps with no usable Nix package; anything
# that ships a real Nix package belongs in home.packages / environment, not here.
{
  homebrew = {
    casks = [
      "1password-cli"
      "beeper"
      "chatgpt"
      "chatgpt-atlas"
      "claude"
      "codex-app"
      "contexts"
      "cursor"
      "emacs-app"
      "ghostty"
      "google-chrome"
      "helium-browser"
      "linear"
      "lm-studio"
      "notion"
      "obs"
      "obsidian"
      "orbstack"
      "postico"
      "prismlauncher"
      "raycast"
      "screen-studio"
      "setapp"
      "signal"
      "skim"
      "slack"
      "stremio"
      "superhuman"
      "superwhisper"
      "tailscale-app"
      "tableplus"
      "todoist-app"
      "jetbrains-toolbox"
      "thebrowsercompany-dia"
      # RealVNC viewer: the ix fleet's headless remote desktop is wayvnc, which
      # offers only RFB security type "None" (no auth); macOS Screen Sharing.app
      # refuses no-auth servers, so a third-party client is required to reach
      # `vnc://<host>.tail368802.ts.net:5900`. See ix nix/modules/desktop/remote-desktop.nix.
      "vnc-viewer"
      "zed"
      "zoom"
    ];

    # `mas` (Mac App Store CLI) is the brew that drives `masApps` below.
    brews = [ "mas" ];

    masApps = {
      "Things 3" = 904280696;
      "Super Easy Timer" = 1353137878;
      "Flighty – Live Flight Tracker" = 1358823008;
      "Apple Configurator 2" = 1037126344;
    };
  };
}
