{
  description = "ix example: fleet-microservices";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    index = {
      url = "github:indexable-inc/index";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = {index, ...}: let
    vms = import ./default.ix {inherit index;};
  in {
    inherit (vms) nixosConfigurations;
  };
}
