{
  description = "ix example: hyperion, one game server behind three proxies";

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
    nixpkgs,
    index,
    hyperion,
    ...
  }: let
    # `default.ix` is JavaScript-syntax Nix, converted during evaluation.
    fleet = index.lib.importIxWasm ./default.ix {inherit index hyperion;};
  in {
    inherit (fleet) nixosConfigurations;
    # One build for the whole fleet. The nodes share 99.8% of their closure, so
    # building them together costs barely more than building one, and `ix apply`
    # then exports the finished system to each VM rather than asking every guest
    # to compile the same thing over again. The proxies are `replicas` of one
    # spec in `default.ix`, so each still needs naming here: raising the count
    # adds a `-system` attr and an apply target, not a node definition.
    #
    #   nix build .#hyperion-game-system .#hyperion-proxy-0-system .#hyperion-proxy-1-system .#hyperion-proxy-2-system
    #   ix apply .#hyperion-game .#hyperion-proxy-0 .#hyperion-proxy-1 .#hyperion-proxy-2
    #
    # Exposed under every system, not only `x86_64-linux`, because these are
    # x86_64-linux guest systems whatever machine you build them from: the
    # machine that types `nix build` contributes a builder, not an identity.
    # Without this the command above is a missing-attribute error on the Mac it
    # is most likely to be typed on.
    packages = nixpkgs.lib.genAttrs [
      "aarch64-darwin"
      "aarch64-linux"
      "x86_64-darwin"
      "x86_64-linux"
    ] (_: fleet.systemPackages);
  };
}
