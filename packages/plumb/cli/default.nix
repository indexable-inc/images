{
  ix,
  lib,
  ...
}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "plumb";
  meta = {
    description = "Inspectable bash-subset shell: runs are values, pipe stages are captured, outputs auto-bind to variables";
    license = lib.licenses.mit;
    mainProgram = "plumb";
  };
}
