{
  ix,
  lib,
  ...
}:
# A tao+wry desktop consumer of the dashboard producer stream. macOS-first: wry
# links the system WebKit framework on darwin; Linux (WebKitGTK) is a later add,
# so the flake output is darwin-only for now.
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "ix-windows";
  meta = {
    description = "Render each live MCP resource as its own borderless native webview window";
    license = lib.licenses.mit;
    mainProgram = "ix-windows";
    platforms = [
      "aarch64-darwin"
      "x86_64-darwin"
    ];
  };
}
