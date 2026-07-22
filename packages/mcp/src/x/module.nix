{
  browserModule,
  bundledSource,
  importTest,
  pkgs,
  privateSessionModule,
}: let
  # Read recent X (Twitter) posts into polars by driving the logged-in browser:
  # `import x`, then `await x.posts("@handle")` / `x.posts("home")` navigates the
  # browser `browser` connects to, scrolls until it has enough tweets, and parses
  # them into a polars frame. Pure Python over the bundled browser/playwright/polars
  # (X has no usable unauthenticated read API); cross-platform.
  xPythonSource = bundledSource {
    name = "ix-mcp-x-python-source";
    path = ./.;
  };
  xModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-x-python-module"
    {
      strictDeps = true;
      passthru.ixFirstPartyDeps = [
        browserModule
        privateSessionModule
      ];
      meta.description = "Read recent X posts to polars via the logged-in browser, bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/x"
      mkdir -p "$site"
      cp -r ${xPythonSource}/x/. "$site/"
    ''
  );
  xBundled = importTest [xModule] "x" "import x; print('x-ok', callable(x.posts), x.__version__)";
in {
  module = xModule;
  tests = {
    inherit
      xBundled
      ;
  };
}
