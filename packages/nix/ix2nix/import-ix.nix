# Import a `.ix` module: convert it to Nix source in-eval through the compiled
# ix2nix plugin (`builtins.wasm`, behind the `wasm-builtin` experimental
# feature of the patched nix-ix evaluator), then `import` the result.
#
# Every converted module renders as `{ __dir, __importIx }: <body>` -- one
# calling convention regardless of whether the source uses `import()` -- so
# this shim applies exactly that attrset: `__dir` anchors the module's
# relative `import()` specifiers and `__importIx` recurses through this same
# function for `.ix` imports.
{
  # Store path of the compiled converter (`ix2nix.wasm`).
  converter,
}: let
  importIx = path: let
    nixSource = builtins.wasm {
      path = converter;
      function = "convert";
    } (builtins.readFile path);
  in
    import (builtins.toFile (baseNameOf path + ".nix") nixSource) {
      __dir = dirOf path;
      __importIx = importIx;
    };
in
  importIx
