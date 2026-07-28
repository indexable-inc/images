{
  description = "ix apply multi-VM switch: several NixOS VMs switched in one command";

  inputs = {
    # https://github.com/indexable-inc/index/issues/1537: every standalone
    # example points at the public Index flake; this one still demonstrates raw
    # NixOS attrs, not the ix VM wrapper, so only the `.ix` converter is used
    # from index.
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    index = {
      url = "github:indexable-inc/index";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = {
    nixpkgs,
    index,
    ...
  }: let
    # `default.ix` is JavaScript-syntax Nix. `builtins.wasm` converts it during
    # evaluation, so evaluating this flake takes index's patched nix with
    # `wasm-builtin` in `extra-experimental-features` (`ix apply` and `ix eval`
    # pass the flag).
    importIx = index.lib.importIxWasm;
    example = importIx ./default.ix {inherit nixpkgs;};
  in {
    inherit (example) nixosConfigurations;
  };
}
