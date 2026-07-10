{
  description = "ix example: _non-nix-oci-debian";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    index = {
      url = "github:indexable-inc/index";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = {index, ...}: let
    image = import ./ix.nix {inherit index;};
  in {
    ix.images.default = image;
    packages.x86_64-linux.default = image;
  };
}
