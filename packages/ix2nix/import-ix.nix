# Import a `.ix` module: convert it to Nix source in-eval through the compiled
# ix2nix plugin (`builtins.wasm`, behind the `wasm-builtin` experimental
# feature of the patched nix-ix evaluator), then `import` the result.
#
# The converter the repo wires in (lib/default.nix, `importIxWasm`) is the
# COMMITTED `lib/ix2nix.wasm`, a plain file in the tree rather than a
# derivation output: importing a `.ix` file realizes no store path mid-eval
# (no IFD, no substitution), so `.ix` evals stay fast and work offline,
# including a fresh `ix init` project's first eval. The artifact lives under
# lib/, NOT next to the wasm crate, because unit-scoped crate sources take
# the whole crate directory: a sibling file would fold the artifact into its
# own build's input hash and the freshness gate could never converge (the
# same reason the retired lib/import-ix-native.nix lived under lib/). The
# committed bytes are pinned to the x86_64-linux build of `.#ix2nix-wasm`
# and a CI gate keeps them in sync with the crate source; see
# `wasm/default.nix`. The wasm detour itself is transitional: once nix has
# proper parallel IFD we hope to drop the wasm converter and run the native
# ix2nix binary instead.
#
# Every converted module renders as `{ __dir, __importIx, __ixTy }: <body>`
# -- one calling convention regardless of whether the source uses `import()`
# or type annotations -- so this shim applies exactly that attrset: `__dir`
# anchors the module's relative `import()` specifiers, `__importIx` recurses
# through this same function for `.ix` imports, and `__ixTy` is the type
# runtime that decides whether emitted annotations check (`assert`) or cost
# nothing (`erase`).
{
  # The compiled converter (`ix2nix.wasm`). A parameter, not a default: the
  # committed artifact lives outside this directory (see the header) and
  # up-paths are banned, so the repo wires it through `paths.root` in
  # lib/default.nix; pre-#4125 scaffolded flakes pass the `ix2nix-wasm`
  # package output they already interpolate, and the e2e passes the freshly
  # built package.
  converter,
  # Type-check mode for every module this shim imports; see `ix-ty.nix`.
  typeMode ? "assert",
}: let
  ixTy = import ./ix-ty.nix {mode = typeMode;};
  # `builtins.wasm` only exists under `wasm-builtin`; forced when a `.ix`
  # file is actually imported, and the throw names the two working setups so
  # a stock-nix eval fails with instructions instead of a missing-attribute
  # error.
  convert =
    if builtins ? wasm
    then
      builtins.wasm {
        path = converter;
        function = "convert";
      }
    else
      throw (
        "importIx: this evaluator has no `builtins.wasm`, so it cannot convert `.ix` modules. "
        + "Run through `ix eval` or `ix apply`, or use the nix-ix client with "
        + "`wasm-builtin` in `extra-experimental-features`."
      );
  importIx = path: let
    # Strings derived from a file inside a flake's realized source tree
    # (`baseNameOf path`, `readFile path`) carry that store path's context,
    # and `toFile` refuses both a name and contents that reference store
    # paths. Drop the context on both: the converted text is self-contained
    # source and the name is just a label, so neither loses a real
    # dependency (the eval depends on the `.ix` file through `readFile`
    # itself, not through the context).
    fileName = builtins.unsafeDiscardStringContext (baseNameOf path + ".nix");
    nixSource = builtins.unsafeDiscardStringContext (convert (builtins.readFile path));
  in
    import (builtins.toFile fileName nixSource) {
      __dir = dirOf path;
      __importIx = importIx;
      __ixTy = ixTy.forModule (toString path);
    };
in
  importIx
