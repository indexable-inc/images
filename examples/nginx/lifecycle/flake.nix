{
  description = "ix example: nginx-lifecycle";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    index = {
      url = "github:indexable-inc/index";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = {index, ...}: let
    vm = import ./default.ix {inherit index;};
  in {
    ix.default = vm;
    inherit (vm) nixosConfigurations;
  };
}
