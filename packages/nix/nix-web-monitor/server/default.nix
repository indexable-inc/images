{
  ix,
  lib,
  pkgs,
}:
let
  fs = lib.fileset;
  siteSrc = fs.toSource {
    root = ./site;
    fileset = fs.intersection (fs.gitTracked ./.) ./site;
  };

  site = ix.buildSvelteSite pkgs {
    pname = "nix-web-monitor-site";
    version = "0.1.0";
    src = siteSrc;
    serve.enable = false;
    devServer = {
      name = "nix-web-monitor-site-dev";
      checkoutSubdir = "packages/nix/nix-web-monitor/server/site";
    };
  };

  unwrapped = ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "nix-web-monitor";
    meta.mainProgram = "nix-web-monitor";
  };

  wrapper =
    pkgs.runCommand "${unwrapped.pname}-${unwrapped.version}"
      {
        strictDeps = true;
        nativeBuildInputs = [ pkgs.makeBinaryWrapper ];
        passthru = {
          tests = unwrapped.passthru.tests // {
            inherit site;
          };
          inherit site unwrapped;
        };
        meta = unwrapped.meta // {
          description = "Run Nix with a live web monitor for logs, builds, and activity DAGs";
          mainProgram = "nix-web-monitor";
        };
      }
      ''
        mkdir -p "$out/bin" "$out/share/nix-web-monitor"
        cp -R ${site}/share/nix-web-monitor-site/. "$out/share/nix-web-monitor/"
        # Deliberately no PATH wrapping for `nix`: the wrapper invokes
        # whatever Nix is already on the operator's PATH so that
        # `nix-web-monitor build .#x` uses the same Nix as a bare
        # `nix build .#x` would. Pinning a specific Nix here drags an
        # extra copy into every closure and silently shadows custom
        # builds. The same reasoning keeps `home-manager`, `darwin-rebuild`,
        # and `sudo` (used by the `home`/`os` switch subcommands) on the
        # operator's PATH rather than bundled.
        #
        # `nvd` is the one exception: the post-switch generation diff should work
        # out of the box, so it is appended with `--suffix` (an operator's own
        # `nvd` earlier on PATH still wins).
        # Stamp the build revision and commit time for `--version`. These ride
        # the wrapper env (not `env!`) and the `build-version` crate renders
        # them at runtime, so a new commit re-stamps this tiny wrapper without
        # rebuilding the Rust unit. `IX_BUILD_*` are the shared names every
        # ix tool reads; see `build-version` and `ix.rev` / `ix.revEpoch`.
        makeWrapper ${lib.getExe unwrapped} "$out/bin/nix-web-monitor" \
          --set NIX_WEB_MONITOR_SITE_DIR "$out/share/nix-web-monitor" \
          --suffix PATH : ${lib.makeBinPath [ pkgs.nvd ]} \
          --set IX_BUILD_REV ${lib.escapeShellArg ix.rev} \
          --set IX_BUILD_EPOCH ${lib.escapeShellArg (toString ix.revEpoch)}
        ln -s nix-web-monitor "$out/bin/nwm"
      '';
in
wrapper
