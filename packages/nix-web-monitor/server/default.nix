{
  ix,
  lib,
  pkgs,
}:
let
  fs = lib.fileset;
  siteSrc = fs.toSource {
    root = ./site;
    fileset = fs.intersection (fs.gitTracked ./.) (
      fs.unions [
        ./site/eslint.config.js
        ./site/index.html
        ./site/package-lock.json
        ./site/package.json
        ./site/src
        ./site/svelte.config.ts
        ./site/tsconfig.json
        ./site/vite.config.js
      ]
    );
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
    pkgs.runCommand "nix-web-monitor-0.1.0"
      {
        strictDeps = true;
        nativeBuildInputs = [ pkgs.makeBinaryWrapper ];
        passthru = {
          tests = (unwrapped.passthru.tests or { }) // {
            inherit site;
          };
          inherit site unwrapped;
        };
        meta = (unwrapped.meta or { }) // {
          description = "Run Nix with a live web monitor for logs, builds, and activity DAGs";
          mainProgram = "nix-web-monitor";
        };
      }
      ''
        mkdir -p "$out/bin" "$out/share/nix-web-monitor"
        cp -R ${site}/share/nix-web-monitor-site/. "$out/share/nix-web-monitor/"
        makeWrapper ${lib.getExe unwrapped} "$out/bin/nix-web-monitor" \
          --set NIX_WEB_MONITOR_SITE_DIR "$out/share/nix-web-monitor" \
          --prefix PATH : ${lib.makeBinPath [ pkgs.nix ]}
      '';
in
wrapper
