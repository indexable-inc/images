{
  bundledSource,
  pkgs,
}: let
  # `sharedaudio`: drive the local shared-audio daemon (packages/audio) over
  # its unix control socket: status, local volume, and publishing WASM
  # instruments / control changes to every peer. Pure stdlib JSON-lines
  # client, cross-platform, so every session can `import sharedaudio`.
  sharedaudioPythonSource = bundledSource {
    name = "ix-mcp-sharedaudio-python-source";
    path = ./.;
  };
  sharedaudioModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-sharedaudio-python-module"
    {
      strictDeps = true;
      meta.description = "shared-audio daemon control client bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/sharedaudio"
      mkdir -p "$site"
      cp -r ${sharedaudioPythonSource}/sharedaudio/. "$site/"
    ''
  );
in {
  module = sharedaudioModule;
}
