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
      "1password-cli@beta"
      "beeper"
      "chatgpt"
      "chatgpt-atlas"
      "claude"
      # Cloudflare WARP: tunnels IPv4+IPv6 to Cloudflare, so an IPv6-only or
      # broken-IPv4 network (e.g. hotel wifi with dead DHCPv4) can still reach
      # IPv4-only hosts like github and Apple's APNs. Needs UDP egress to work.
      "cloudflare-warp@beta"
      "codex-app"
      "contexts"
      "cursor"
      "emacs-app@nightly"
      "ghostty@tip"
      "google-chrome"
      "helium-browser"
      "linear"
      "lm-studio"
      "mullvad-vpn@beta"
      "notion"
      "obs@beta"
      "obsidian"
      "postico"
      "prismlauncher"
      "raycast"
      "screen-studio"
      "setapp"
      "signal@beta"
      "skim"
      "slack@beta"
      "spotify"
      "stremio@beta"
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
      "zed@preview"
      "zoom"
    ];

    # `mas` (Mac App Store CLI) is the brew that drives `masApps` below.
    brews = ["mas"];

    # Every Mac App Store app installed on the workstation must be listed here:
    # onActivation.cleanup = "zap" uninstalls any MAS app not declared, so an
    # omission deletes the app on the next switch (it lost Final Cut/Logic/Xcode
    # once before this list was completed). IDs come from `mas list`.
    masApps = {
      "Things 3" = 904280696;
      "Super Easy Timer" = 1353137878;
      "Flighty – Live Flight Tracker" = 1358823008;
      "Apple Configurator 2" = 1037126344;
      "Final Cut Pro" = 424389933;
      "Logic Pro" = 634148309;
      "GarageBand" = 682658836;
      "iMovie" = 408981434;
      "Xcode" = 497799835;
      "TestFlight" = 899247664;
      "Apple Developer" = 640199958;
      "Fantastical" = 975937182;
      "WireGuard" = 1451685025;
      "Pages" = 409201541;
      "Numbers" = 409203825;
      "Keynote" = 409183694;
      "Portal" = 1436994560;
      "Microsoft Word" = 462054704;
      "Microsoft Excel" = 462058435;
      "Microsoft PowerPoint" = 462062816;
      "Microsoft Outlook" = 985367838;
    };
  };
}
