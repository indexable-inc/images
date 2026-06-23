{
  description = "ix example: nomad-secret-refs";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    index = {
      url = "github:indexable-inc/index";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { index, ... }:
    let
      example = import ./ix.nix { inherit index; };
    in
    {
      ix.examples.default = example;
      checks.x86_64-linux.default = example.buildCheck;
    };
}
