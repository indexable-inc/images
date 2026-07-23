{
  description = "ix example: kernel-build";

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
    importIx = import (index + "/packages/ix2nix/import-ix.nix") {
      converter = "${index.packages.${index.lib.system}.ix2nix-wasm}/lib/ix2nix.wasm";
    };
    vm = importIx ./default.ix {inherit index;};
  in {
    ix.default = vm;
    inherit (vm) nixosConfigurations;
  };
}
