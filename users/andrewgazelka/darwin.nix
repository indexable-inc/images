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
{lib, ...}: let
  # Shared verified name -> MAS ID catalog; this module only picks which apps
  # this user installs. `lib.getAttrs` throws on an unknown name, so a typo
  # here is an eval error instead of a silent zap-uninstall.
  masCatalog = import ../../lib/darwin/mas-apps.nix;
in {
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
      # `vnc://<host>.<tailnet>.ts.net:5900`. See ix nix/modules/desktop/remote-desktop.nix.
      "vnc-viewer"
      "zed@preview"
      "zoom"
    ];

    # `mas` (Mac App Store CLI) is the brew that drives `masApps` below.
    brews = ["mas"];

    # Every Mac App Store app installed on the workstation must be listed here:
    # onActivation.cleanup = "zap" uninstalls any MAS app not declared, so an
    # omission deletes the app on the next switch (it lost Final Cut/Logic/Xcode
    # once before this list was completed). IDs live in the shared catalog
    # (lib/darwin/mas-apps.nix); this list is just the selection.
    masApps =
      lib.getAttrs [
        "Things 3"
        "Super Easy Timer"
        "Flighty – Live Flight Tracker"
        "Apple Configurator 2"
        "Final Cut Pro"
        "Logic Pro"
        "GarageBand"
        "iMovie"
        "Xcode"
        "TestFlight"
        "Apple Developer"
        "Fantastical"
        "WireGuard"
        "Pages"
        "Numbers"
        "Keynote"
        "Portal"
        "Microsoft Word"
        "Microsoft Excel"
        "Microsoft PowerPoint"
        "Microsoft Outlook"
      ]
      masCatalog;
  };
}
