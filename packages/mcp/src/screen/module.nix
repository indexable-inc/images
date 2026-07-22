{
  bundledSource,
  importTest,
  lib,
  pkgs,
}: let
  # Native macOS screen capture and cursor control, bundled like `tui` and
  # `search` so every session can `import screen`. This one is pure Python (no
  # PyO3 cdylib): it wraps the Apple-maintained pyobjc `Quartz` binding for
  # capture and synthetic input, and probes `AXIsProcessTrusted()` through
  # ctypes for the Accessibility (TCC) permission check. macOS-only: the module
  # itself raises on a non-Darwin platform, and `Quartz` is not available off
  # Darwin, so the dependency is gated in default.nix.
  screenPythonSource = bundledSource {
    name = "ix-mcp-screen-python-source";
    path = ./.;
  };
  screenModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-screen-python-module"
    {
      strictDeps = true;
      meta.description = "Native macOS screen/cursor helper bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/screen"
      mkdir -p "$site"
      cp -r ${screenPythonSource}/screen/. "$site/"
    ''
  );
  screenBundled = importTest [screenModule] "screen" "import screen; print('screen-ok', all(callable(getattr(screen, n)) for n in ('capture', 'click', 'write', 'press', 'key_down', 'key_up', 'apps', 'frontmost', 'launch', 'activate', 'terminate', 'accessibility_trusted')))";
in {
  module = screenModule;
  darwinOnly = true;
  tests = lib.optionalAttrs pkgs.stdenv.hostPlatform.isDarwin {
    inherit
      screenBundled
      ;
  };
}
