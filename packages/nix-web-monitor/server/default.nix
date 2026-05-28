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
      checkoutSubdir = "packages/nix-web-monitor/server/site";
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
        # builds.
        makeWrapper ${lib.getExe unwrapped} "$out/bin/nix-web-monitor" \
          --set NIX_WEB_MONITOR_SITE_DIR "$out/share/nix-web-monitor"
      '';
in
wrapper
