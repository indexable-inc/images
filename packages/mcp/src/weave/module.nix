{
  bundledSource,
  pkgs,
}: let
  # Weave 2 async client: facts, queries, blobs, chat, and delegation verbs
  # against the shared Weave journal. Pure Python over bundled httpx + polars.
  weavePythonSource = bundledSource {
    name = "ix-mcp-weave-python-source";
    path = ./.;
  };
  weaveModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-weave-python-module"
    {
      strictDeps = true;
      # The weave client's unit tests run inside fabricTests
      # (src/fabric/module.nix), which shares their httpx.MockTransport env;
      # the file rides this drv as passthru so fabric needs no cross-directory
      # path literal. passthru does not enter the derivation hash.
      passthru.ixTestSource = builtins.path {
        name = "ix-mcp-weave-test";
        path = ./test_weave.py;
      };
      meta.description = "Weave 2 async client bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/weave"
      mkdir -p "$site"
      cp -r ${weavePythonSource}/weave/. "$site/"
    ''
  );
in {
  module = weaveModule;
}
