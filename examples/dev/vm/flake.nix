{
  description = "ix example: dev-vm";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    index = {
      url = "github:indexable-inc/index";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = {
    self,
    index,
    ...
  }: let
    # `default.ix` is JavaScript-syntax Nix. `builtins.wasm` converts it during
    # evaluation, so evaluating this flake takes index's patched nix with
    # `wasm-builtin` in `extra-experimental-features` (`ix apply` and `ix eval`
    # pass the flag).
    importIx = index.lib.importIxWasm;
    vm = importIx ./default.ix {
      inherit index;
      src = self;
    };
  in {
    # No `ix.default`. `mkDev` layers over `mkFleet`, so `vm` is a fleet result
    # and has no `config`. A bare `ix apply` prefers a flake's `ix.default` and
    # builds `ix.default.config.system.build.toplevel` from it, so binding the
    # fleet result here fails the apply on a missing attribute instead of
    # converging the nodes below.
    inherit (vm) nixosConfigurations;
  };
}
