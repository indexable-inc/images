{
  ix,
  lib,
  ...
}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "ix-credential";
  meta = {
    description = "Lend a GitHub credential to a remote host over a forwarded unix socket, for the life of one ssh session";
    license = lib.licenses.mit;
    mainProgram = "ix-credential";
  };
}
