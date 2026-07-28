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
    # One build for the whole fleet. The three nodes share 99.8% of their
    # closure, so building them together costs barely more than building one,
    # and `ix apply` then exports the finished system to each VM rather than
    # asking three guests to compile the same thing three times:
    #
    #   nix build .#hyperion-game-system .#hyperion-proxy-0-system .#hyperion-proxy-1-system
    #   ix apply .#hyperion-game .#hyperion-proxy-0 .#hyperion-proxy-1
    #
    # Exposed under every system, not only `x86_64-linux`, because these are
    # x86_64-linux guest systems whatever machine you build them from: the
    # machine that types `nix build` contributes a builder, not an identity.
    # Without this the command above is a missing-attribute error on the Mac it
    # is most likely to be typed on.
    packages = builtins.listToAttrs (map (system: {
        name = system;
        value = fleet.systemPackages;
      }) [
        "aarch64-darwin"
        "aarch64-linux"
        "x86_64-darwin"
        "x86_64-linux"
      ]);
  };
}
