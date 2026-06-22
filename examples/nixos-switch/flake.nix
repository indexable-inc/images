{
  description = "Native ix up NixOS switch example";

  inputs.index.url = "github:indexable-inc/index";

  outputs =
    { index, ... }:
    let
      fleet = import ./default.nix { inherit index; };
    in
    {
      # `ix up .#devbox` resolves this to
      # nixosConfigurations.devbox.config.system.build.toplevel.
      inherit (fleet) nixosConfigurations;
    };
}
