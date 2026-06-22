{
  description = "ix up multi-VM switch: one build VM, several NixOS VMs switched in one command";

  inputs = {
    # https://github.com/indexable-inc/index/issues/1537: every standalone
    # example points at the public Index flake; this one still demonstrates raw
    # NixOS attrs, not mkFleet.
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    index = {
      url = "github:indexable-inc/index";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { nixpkgs, ... }:
    let
      example = import ./ix.nix { inherit nixpkgs; };
    in
    {
      inherit (example) nixosConfigurations;
      ix.nixosConfigurations.default = example.nixosConfigurations;
    };
}
