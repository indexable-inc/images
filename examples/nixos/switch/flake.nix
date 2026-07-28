{
  description = "ix example: nixos-switch";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    index = {
      url = "github:indexable-inc/index";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = {index, ...}: let
    # `default.ix` is JavaScript-syntax Nix. `builtins.wasm` converts it during
    # evaluation, so evaluating this flake takes index's patched nix with
    # `wasm-builtin` in `extra-experimental-features` (`ix apply` and `ix eval`
    # pass the flag).
    importIx = index.lib.importIxWasm;
    vm = importIx ./default.ix {inherit index;};
  in {
    ix.default = vm;
    inherit (vm) nixosConfigurations;
  };
}
