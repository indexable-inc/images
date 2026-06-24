# wrangler CLI from the published npm package.
#
# nixpkgs builds wrangler from the workers-sdk source tree, and that build
# fails deterministically on darwin (tsup's DTS step dies with EBADF under
# the sandbox). The npm package already contains the built dist/, so this
# installs it from the lockfile pin in this directory with the same
# importNpmLock machinery the web trees use, and wraps the bin with Node.
{
  ix,
  pkgs,
}:
let
  inherit (pkgs) lib;
  packageJson = lib.importJSON ./package.json;
  nodeModules = pkgs.importNpmLock.buildNodeModules {
    npmRoot = ./.;
    nodejs = pkgs.nodejs_22;
    derivationArgs = {
      pname = "wrangler-cli-node-modules";
      version = packageJson.dependencies.wrangler;
      strictDeps = true;
    };
  };
in
ix.writeBashApplication pkgs {
  name = "wrangler";
  runtimeInputs = [ pkgs.nodejs_22 ];
  text = ''
    exec node ${nodeModules}/node_modules/wrangler/bin/wrangler.js "$@"
  '';
  meta.description = "Cloudflare wrangler CLI (npm dist, not built from source)";
}
