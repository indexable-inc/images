# Import a `.ix` module: convert it to Nix source in-eval through the compiled
# ix2nix plugin (`builtins.wasm`, behind the `wasm-builtin` experimental
# feature of the patched nix-ix evaluator), then `import` the result.
#
# Every converted module renders as `{ __dir, __importIx, __ixTy }: <body>`
# -- one calling convention regardless of whether the source uses `import()`
# or type annotations -- so this shim applies exactly that attrset: `__dir`
# anchors the module's relative `import()` specifiers, `__importIx` recurses
# through this same function for `.ix` imports, and `__ixTy` is the type
# runtime that decides whether emitted annotations check (`assert`) or cost
# nothing (`erase`).
{
  # Store path of the compiled converter (`ix2nix.wasm`).
  converter,
  # Type-check mode for every module this shim imports; see `ix-ty.nix`.
  typeMode ? "assert",
}: let
  ixTy = import ./ix-ty.nix {mode = typeMode;};
  importIx = path: let
    nixSource = builtins.wasm {
      path = converter;
      function = "convert";
    } (builtins.readFile path);
  in
    import (builtins.toFile (baseNameOf path + ".nix") nixSource) {
      __dir = dirOf path;
      __importIx = importIx;
      __ixTy = ixTy.forModule (toString path);
    };
in
  importIx
