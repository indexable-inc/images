{
  description = "ix example: dev-fleet";

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
    fleet = import ./ix.nix {
      inherit index;
      src = self;
    };
  in {
    ix.fleets.default = fleet;
    inherit (fleet) nixosConfigurations;
  };
}
