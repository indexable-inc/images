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
  # This module is exported as a bare path (flake `darwinModules.andrewgazelka`)
  # so external darwin hosts can import it without `ix` in their module args;
  # the catalog can only be reached relatively here.
  # astlog-ignore: no-parent-path
  masCatalog = import ../../lib/darwin/mas-apps.nix;
in {
  homebrew = {
    # WARNING: the consuming host runs `brew bundle --force-cleanup --zap` on
    # activation (see hosts/hydra/default.nix), which UNINSTALLS any cask not in
    # this list AND any cask whose declared name doesn't match what's installed.
    # A pre-release suffix is a DIFFERENT cask: `ghostty@tip` != `ghostty`,
    # `slack@beta` != `slack`. Declare the suffixed channel but have the stable
    # app installed (or vice-versa) and the next `darwin-rebuild switch` zaps the
    # running app out from under you. (#1303 flipped these to @beta/@tip/@nightly
    # and so zapped stable ghostty.) Keep entries stable (no `@channel`) unless
    # you also install that exact channel locally so declared == installed.
    casks = [
      "1password-cli"
      "beeper"
      "chatgpt"
      "chatgpt-atlas"
      "claude"
      # Cloudflare WARP: tunnels IPv4+IPv6 to Cloudflare, so an IPv6-only or
      # broken-IPv4 network (e.g. hotel wifi with dead DHCPv4) can still reach
      # IPv4-only hosts like github and Apple's APNs. Needs UDP egress to work.
      "cloudflare-warp"
      # Ghostty-based terminal with programmable browser panes (cmux.com);
      # the agent rules open response HTML as a cmux browser split.
      "cmux"
      "codex-app"
      "contexts"
      "cursor"
      "emacs-app"
      "ghostty"
      "google-chrome"
      "helium-browser"
      "linear"
      "lm-studio"
      "mullvad-vpn"
      "notion"
      "obs"
      "obsidian"
      "postico"
      "prismlauncher"
      "raycast"
      "screen-studio"
      "setapp"
      "signal"
      "skim"
      "slack"
      "spotify"
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
      # `vnc://<host>.<tailnet>.ts.net:5900`. See ix nix/modules/desktop/remote-desktop.nix.
      "vnc-viewer"
      # Zed via brew: the nix package (crane-built fork) was dropped for eval-time
      # fetch storms (index#4028); the cask sidesteps the build entirely.
      "zed"
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
