{
  ix,
  lib,
  ...
}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "net-trace";
  meta = {
    description = "Wrap a command with a recording localhost proxy and report its client-side network activity";
    license = lib.licenses.mit;
    mainProgram = "net-trace";
  };
}
