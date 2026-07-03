{
  ix,
  lib,
  ...
}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "config-launch";
  meta = {
    description = "Spec-driven exec launcher: set env/PATH and inject CLI flags (static, argv-conditional, and config-file-gated --config) then exec a target, preserving argv0";
    license = lib.licenses.mit;
    mainProgram = "config-launch";
  };
}
