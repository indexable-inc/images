{
  ix,
  pkgs,
}: let
  site = ix.buildSvelteSite pkgs {
    sourceRoot = ./site;
    serve.enable = false;
  };

  server = ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "nix-web-monitor";
    meta.mainProgram = "nix-web-monitor";
  };
in
  # The Rust server and the Svelte site are built independently, then composed:
  # the site is installed as a resource and its path is handed to the server via
  # `NIX_WEB_MONITOR_SITE_DIR`. The cross lane swaps `ix.rustWorkspace` and
  # `ix.wrapPackage` underneath, so this definition is target-agnostic.
  #
  # `nix` stays on the operator's PATH so `nix-web-monitor build .#x` uses the
  # same Nix as a bare `nix build .#x`. `nvd` is the one bundled helper (native
  # only): the post-switch generation diff should work out of the box, and an
  # operator's own earlier PATH entry still wins.
  ix.wrapPackage pkgs {
    package = server;
    resources.site = {
      source = site;
      from = "share/nix-web-monitor-site";
      to = "share/nix-web-monitor";
      env = "NIX_WEB_MONITOR_SITE_DIR";
    };
    nativePathSuffix = [pkgs.nvd];
    # Stamp the build revision and commit time for `--version`. These ride the
    # wrapper env (not `env!`) and the `build-version` crate renders them at
    # runtime, so a new commit re-stamps this tiny wrapper without rebuilding
    # the Rust unit. `IX_BUILD_*` are the shared names every ix tool reads; see
    # `build-version` and `ix.rev` / `ix.revEpoch`.
    env = {
      IX_BUILD_REV = ix.rev;
      IX_BUILD_EPOCH = toString ix.revEpoch;
    };
    symlinks.nwm = "nix-web-monitor";
    passthru = {
      tests =
        server.passthru.tests
        // {
          inherit site;
        };
      inherit site;
    };
    meta = {
      description = "Run Nix with a live web monitor for logs, builds, and activity DAGs";
      mainProgram = "nix-web-monitor";
    };
  }
