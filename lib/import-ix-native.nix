# Import a `.ix` module on an evaluator WITHOUT `builtins.wasm` (stock nix):
# convert it to Nix source in a tiny derivation running the compiled `ix2nix`
# binary, then `import` the output (import-from-derivation). The repo's own
# example discovery and tests load through this shim because CI evaluates the
# flake with stock Determinate Nix; scaffolded user projects keep loading
# through the in-eval wasm shim, `packages/ix2nix/import-ix.nix`, instead
# (`ix apply`/`ix eval` run index's patched nix, no IFD).
#
# Lives under lib/, not next to that wasm twin, because the crate's source
# filter takes the whole crate directory: a sibling file would re-key the
# converter derivations and lose their cache hits for every edit here.
#
# Same calling convention as the wasm shim: every converted module renders as
# `{ __dir, __importIx, __ixTy }: <body>`, so `__dir` anchors the module's
# relative `import()` specifiers, `__importIx` recurses through this same
# function for `.ix` imports, and `__ixTy` runs the emitted type checks.
{
  # Package set the conversion derivation builds with; must match the
  # evaluating system so the IFD can build locally.
  pkgs,
  # The compiled converter CLI (`packages.<system>.ix2nix`).
  ix2nix,
  # The imported type runtime (`packages/ix2nix/ix-ty.nix`, applied to a
  # mode); threaded in because up-paths out of lib/ are banned.
  ixTy,
}: let
  importIx = path: let
    nixSource =
      pkgs.runCommand (baseNameOf path + ".nix") {
        nativeBuildInputs = [ix2nix];
      } ''
        ix2nix < ${path} > $out
      '';
  in
    import nixSource {
      __dir = dirOf path;
      __importIx = importIx;
      __ixTy = ixTy.forModule (toString path);
    };
in
  importIx
