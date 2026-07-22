# The flake's homeModules / darwinModules composition (#3899): shared
# home-module instances, the per-module wiring, and the personal profile
# surfaces (via lib/profiles.nix), kept out of flake.nix so the top level
# reads as a manifest. `indexPackages` is the per-system flake package set
# the modules resolve their tools from.
{
  lib,
  ix,
  paths,
  indexPackages,
  home-manager,
  nixpkgs,
}: let
  # One instance shared by every wiring site (the workstation profile and
  # homeModules.provenance); the module's `key` also dedups the instances a
  # consumer combines, but there is no reason to make them re-apply the
  # walker.
  provenanceHomeModule = import (paths.modules + "/home/provenance.nix") {inherit (ix) provenance;};
  mutableFilesHomeModule = import (paths.modules + "/home/mutable-files.nix") {
    inherit indexPackages;
    portableServicesModule = ix.portableServices.homeModule;
  };
  # Declarative in-guest state push for vmkit macOS guest VMs (launchd
  # agents from structured attrs, pinned binaries, idempotent ssh apply).
  # One instance shared by homeModules.macos-guests and the personal darwin
  # profile. See modules/home/macos-guests.nix.
  macosGuestsHomeModule = import (paths.modules + "/home/macos-guests.nix") {
    inherit indexPackages ix;
  };
  claudeCodeHomeModule = import (paths.packagesRoot + "/agent/home-manager/claude-code.nix") {
    inherit indexPackages;
    promptModule = paths.packagesRoot + "/agent/prompt";
    mutableJsonModule = ix.mutableJson.homeModule;
  };
  codexHomeModule = import (paths.packagesRoot + "/agent/home-manager/codex.nix") {
    inherit indexPackages;
    promptModule = paths.packagesRoot + "/agent/prompt";
  };
  personal = import ./profiles.nix {
    inherit lib ix paths indexPackages home-manager nixpkgs;
    claudeCodeModule = claudeCodeHomeModule;
    codexModule = codexHomeModule;
    mutableFilesModule = mutableFilesHomeModule;
    provenanceModule = provenanceHomeModule;
    macosGuestsModule = macosGuestsHomeModule;
  };
in {
  personalLightProfile = personal.lightProfileFor;
  darwinModules = {
    # Personal-but-shareable nix-darwin module for github:andrewgazelka: the
    # Homebrew package set (GUI casks, the `mas` brew, Mac App Store apps).
    # Companion to homeModules.andrewgazelka (which owns the home-manager
    # services); import it from a darwin host to get the casks merged in. See
    # users/andrewgazelka/darwin.nix.
    andrewgazelka = paths.users + "/andrewgazelka/darwin.nix";
    # Per-generation provenance manifest for nix-darwin: bake deployed-path
    # -> defining nix file:line backlinks (provenance.json) into the system
    # closure so `whence </etc/...>` answers from /run/current-system with
    # zero eval. Set `provenance.rev = self.rev or self.dirtyRev or null`
    # in the consuming flake. See modules/darwin/provenance.nix.
    provenance = import (paths.modules + "/darwin/provenance.nix") {inherit (ix) provenance;};
    # System-level (root, /etc) adapter for declarative-but-writable files:
    # same model as homeModules.mutable-files, state under
    # /var/db/index-delta, boot-time reseed daemon. See
    # modules/darwin/mutable-files.nix.
    mutable-files = import (paths.modules + "/darwin/mutable-files.nix") {
      inherit indexPackages;
    };
    # Fabric Ray worker for macs (index#3192): join the fleet cluster as a
    # worker behind `services.ix-ray.enable`, same pinned ports and env as
    # the NixOS module. See modules/darwin/ray.nix.
    ray = import (paths.modules + "/darwin/ray.nix") {indexLib = ix;};
    # Declarative NFS automounts via macOS autofs: each entry renders a
    # direct-map line, /etc/auto_master gains the include idempotently, and
    # activation reloads automountd. See modules/darwin/nfs.nix.
    nfs = paths.modules + "/darwin/nfs.nix";
  };
  homeModules = {
    # Workstation-facing home-manager module: declare a service once, get a
    # native launchd agent on macOS and native systemd user units on Linux.
    portable-services = ix.portableServices.homeModule;
    tmux = paths.modules + "/home/tmux.nix";
    # Shared modern-CLI package baseline (bat, delta, eza, fd, ripgrep, ...).
    # Import it and set `cliBaseline.enable = true`; override
    # `cliBaseline.packages` to trim or swap tools. See
    # modules/home/cli-baseline.nix.
    cli-baseline = paths.modules + "/home/cli-baseline.nix";
    # Per-project nvim-server multiplexer (tmux replacement): one headless
    # nvim server per git root, `mux` attaches with --remote-ui, and the
    # optional zsh integration makes bare `ssh <host>`/`mosh <host>`
    # auto-attach the remote's mux. Import it and set
    # `programs.mux.enable = true`; needs an nvim config shipping a `mux`
    # lua module. See modules/home/mux.nix.
    mux = import (paths.modules + "/home/mux.nix") {inherit ix;};
    # XDG hygiene: point tool state/caches/config (cargo, go, npm/pnpm,
    # python, docker, aws, psql/sqlite histories, wget/less) at the XDG
    # base directories instead of $HOME. Import it and set
    # `xdgTidy.enable = true`. See modules/home/xdg-tidy.nix.
    xdg-tidy = paths.modules + "/home/xdg-tidy.nix";

    # Wall time on every `Activating <name>` line plus a slowest-steps
    # summary at the end of activation. See
    # modules/home/activation-timing.nix.
    activation-timing = paths.modules + "/home/activation-timing.nix";
    # Cursor-shape feedback for zsh vi mode (beam insert, block command,
    # reset around every prompt/command). Import it and set
    # `zshViCursor.enable = true`. See modules/home/zsh-vi-cursor.nix.
    zsh-vi-cursor = paths.modules + "/home/zsh-vi-cursor.nix";
    # Declarative-but-writable JSON config files (last-applied 3-way merge),
    # for config an app rewrites at runtime. See lib/mutable-json.nix.
    # Prefer `mutable-files` below for new config: it never auto-merges,
    # covers more formats, and queues drift for explicit resolution.
    mutable-json = ix.mutableJson.homeModule;
    # Declarative-but-writable files with logical (format-aware) drift
    # tracking and a model-oriented resolution queue -- no auto-merge.
    # Declared content seeds a plain writable file; ephemeral files reset
    # at login (drift journaled), durable files queue base-vs-drift
    # conflicts in `index-delta status --json` for discard / adopt /
    # absorb-into-Nix via `index-delta apply-ops`. See
    # modules/home/mutable-files.nix and packages/index-delta.
    mutable-files = mutableFilesHomeModule;
    # Reusable workstation module (macOS): declare vmkit macOS guest VMs
    # (ssh endpoint, launchd agents from structured attrs, pinned binaries)
    # and get a `macos-guest-<name>` apply/status/ssh command per guest.
    # Import it and set `macosGuests.<name> = { ssh = ...; ... }`. Manual
    # TCC bootstrap: modules/home/macos-guests/tcc-bootstrap.md. See
    # modules/home/macos-guests.nix (index#3206, toward index#2682).
    macos-guests = macosGuestsHomeModule;
    # Reusable workstation module (macOS): declare Raycast Focus session
    # defaults (title, filter mode, duration) and have them written to the
    # com.raycast.macos defaults domain at switch time. Import it and set
    # `programs.raycast.focus = { enable = true; ... }`. See
    # modules/home/raycast.nix.
    raycast = paths.modules + "/home/raycast.nix";
    # Per-generation provenance manifest: every home-manager generation
    # carries provenance.json mapping deployed files back to the nix
    # file:line that defined them, and `whence <path>` reads it with zero
    # eval. Set `provenance.rev = self.rev or self.dirtyRev or null` in
    # the consuming flake. See modules/home/provenance.nix.
    provenance = provenanceHomeModule;
    # Agent CLI modules: Home Manager is the user-facing configuration
    # surface, while the package wrappers remain the implementation detail.
    claude-code = claudeCodeHomeModule;
    codex = codexHomeModule;
    # Personal-but-shareable workstation module for github:andrewgazelka: the
    # ix.dev downtime watcher + boss bar overlay + the shared say-detached
    # sound helper, all as portable services. Closed over the per-system
    # flake packages so it resolves bossbar / minecraft-sound for the host it
    # runs on. See users/andrewgazelka/home.nix.
    andrewgazelka-portable = personal.portableModule;
    andrewgazelka-development = personal.developmentModule;
    andrewgazelka-workstation = personal.workstationModule;
    andrewgazelka-darwin = personal.darwinHomeModule;
    # Personal-but-shareable server module for github:harivansh-afk: the
    # dotfiles hari runs as the `hari` user on hari-compute-1 (zsh, git,
    # neovim plus the mux nvim multiplexer, and the CLI tool set around
    # them), ported from his personal nix repo. Consumes the shared
    # cli-baseline, mux, xdg-tidy, and zsh-vi-cursor modules above; the
    # source repo's secrets/theme machinery is deliberately absent. See
    # users/harivansh-afk/home.nix.
    harivansh-afk = import (paths.users + "/harivansh-afk/home.nix") {inherit ix;};
    # Reusable workstation module: draw one Minecraft boss bar per in-flight
    # GitHub Actions run across a set of repos (green = running, filled by
    # elapsed / average duration; purple = queued/unpicked). Import it and set
    # `services.ciBars = { enable = true; repos = [ ... ]; }`. Closed over the
    # per-system packages so it resolves the `bossbar` CLI for the host. See
    # packages/minecraft/bossbar-overlay/ci-bars-home-module.nix.
    ci-bars = import (paths.packagesRoot + "/minecraft/bossbar-overlay/ci-bars-home-module.nix") {
      inherit indexPackages ix;
      portableServicesModule = ix.portableServices.homeModule;
    };
    # Workstation-facing module to sync corpus sources (agent/shell history,
    # Slack/Linear exports, git repos) to an S3/R2 parquet archive and/or
    # Mixedbread, as a portable timer service. Closed over the per-system
    # packages so it resolves the `indexer` for the host. See
    # packages/search/indexer/home-module.nix.
    indexer = import (paths.packagesRoot + "/search/indexer/home-module.nix") {
      inherit indexPackages;
      portableServicesModule = ix.portableServices.homeModule;
    };
  };
}
