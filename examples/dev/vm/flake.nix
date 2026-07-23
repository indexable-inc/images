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
    vm = import ./default.ix {
      inherit index;
      src = self;
    };
  in {
    ix.default = vm;
    inherit (vm) nixosConfigurations;
  };
}
