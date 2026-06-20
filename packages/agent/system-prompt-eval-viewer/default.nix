{
  ix,
  lib,
  buildNpmPackage,
  python3,
  coreutils,
}:

let
  # The Svelte + Vite single-page app, built to a static bundle. Only the source
  # files go into the build (no node_modules / dist), and npmDepsHash pins the
  # offline npm dependency closure.
  site = buildNpmPackage {
    pname = "system-prompt-eval-viewer-site";
    version = "0.1.0";
    src = lib.fileset.toSource {
      root = ./.;
      fileset = lib.fileset.unions [
        ./package.json
        ./package-lock.json
        ./vite.config.ts
        ./svelte.config.js
        ./tsconfig.json
        ./index.html
        ./src
      ];
    };
    npmDepsHash = "sha256-Y7hsx0h2lMVTSsNYMXfi6TCqc7qBQH6poKptG2HPHfA=";
    installPhase = ''
      runHook preInstall
      cp -r dist $out
      runHook postInstall
    '';
    meta = {
      description = "Static bundle for the system-prompt eval viewer";
      license = lib.licenses.mit;
    };
  };
in
# `nix run .#system-prompt-eval-viewer -- <result.json>` copies the built site to
# a temp dir, drops the JSON in as data.json (which the app fetches on load),
# serves it, and opens a browser. Without an argument it shows the bundled sample.
# `ix.writeNushellApplication` is curried on the full package set; read it from
# `ix.pkgs` rather than a `pkgs` callPackage formal (unreachable by `override`).
ix.writeNushellApplication ix.pkgs {
  name = "system-prompt-eval-viewer";
  runtimeInputs = [
    python3
    coreutils
  ];
  text = ''
    def main [json?: string] {
      let d = (^mktemp -d | str trim)
      ^cp -r ${site}/. $d
      if $json != null {
        ^cp $json $"($d)/data.json"
      }
      let url = "http://127.0.0.1:8777/"
      print $"serving the eval scorecard at ($url)  -- ctrl-c to stop"
      ^open $url
      ^python3 -m http.server 8777 --bind 127.0.0.1 --directory $d
    }
  '';
}
