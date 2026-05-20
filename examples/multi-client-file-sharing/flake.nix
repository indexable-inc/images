{
  inputs.index.url = "github:indexable-inc/index";

  outputs =
    { index, ... }:
    let
      fleet = import ./default.nix { inherit index; };
    in
    {
      packages.x86_64-linux = fleet.packages;
    };
}
