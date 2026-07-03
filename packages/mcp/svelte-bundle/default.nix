# svelte-bundle: compile a Svelte 5 component into one self-contained IIFE
# bundle for the sandboxed resource iframe (opaque origin, no network), with
# the virtual `ix` module wired to window.ix / window.__IX_STATE__.
#
# Deps come from the lockfile pin in this directory via the same importNpmLock
# machinery the web trees use (see packages/wrangler-cli); cli.mjs and ix.js
# are ours and run with Node from the store.
{
  ix,
  pkgs,
}:
let
  nodeModules = pkgs.importNpmLock.buildNodeModules {
    npmRoot = ./.;
    nodejs = pkgs.nodejs_22;
    derivationArgs = {
      pname = "svelte-bundle-node-modules";
      strictDeps = true;
    };
  };
  source = pkgs.runCommand "svelte-bundle-src" { strictDeps = true; } ''
    mkdir -p "$out"
    cp ${./cli.mjs} "$out/cli.mjs"
    cp ${./ix.js} "$out/ix.js"
    ln -s ${nodeModules}/node_modules "$out/node_modules"
  '';
in
ix.writeBashApplication pkgs {
  name = "svelte-bundle";
  runtimeInputs = [ pkgs.nodejs_22 ];
  text = ''
    exec node ${source}/cli.mjs "$@"
  '';
  meta.description = "Svelte 5 component -> one self-contained IIFE bundle for dashboard resources";
}
