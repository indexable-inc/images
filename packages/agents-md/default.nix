{
  ix,
  pkgs ? ix.pkgs,
}:

let
  unwrapped = ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "agents-md";
    meta.mainProgram = "agents-md";
  };
  package = ix.agentContext.mkApp {
    inherit pkgs;
    binary = unwrapped;
  };
in
package.overrideAttrs (old: {
  passthru =
    (old.passthru or { })
    // (unwrapped.passthru or { })
    // {
      inherit unwrapped;
    };
})
