{
  description = "ix example: hyperion, one game server behind two proxies";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    index = {
      url = "github:indexable-inc/index";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    # Deliberately not following nixpkgs. hyperion builds against its own
    # pinned rust-overlay and nixpkgs, and a follow here swaps the toolchain
    # under a nightly-only workspace.
    hyperion.url = "github:hyperion-mc/hyperion";
  };

  outputs = {
    index,
    hyperion,
    ...
  }: let
    # `default.ix` is JavaScript-syntax Nix, converted during evaluation.
    fleet = index.lib.importIxWasm ./default.ix {inherit index hyperion;};
  in {
    inherit (fleet) nixosConfigurations;
  };
}
