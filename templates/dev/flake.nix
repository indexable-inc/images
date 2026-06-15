{
  description = "My ix dev environment (RFC 0007)";

  inputs.index.url = "github:indexable-inc/index";

  outputs =
    { self, index, ... }:
    let
      # Systems you build VMs from. ix VM closures are linux; add darwin only if
      # you evaluate from a Mac.
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forEach =
        f:
        builtins.listToAttrs (
          map (system: {
            name = system;
            value = f system;
          }) systems
        );
    in
    {
      # `nix run .#up` brings up the fleet (or the single `dev` VM if no fleet is
      # declared); `.#health` / `.#diff` / `.#down` mirror `ix fleet <sub>`.
      # `<node>-system` is each node's system closure.
      #
      # `module = ./dev.nix` is your config. `src = self` is the source
      # materialized at /ix on every node so a VM can rebuild itself - keep it.
      packages = forEach (
        system:
        let
          dev = index.lib.mkDevFor system {
            module = ./dev.nix;
            src = self;
          };
        in
        {
          default = dev.up;
          inherit (dev)
            up
            health
            diff
            down
            ;
        }
        // dev.systemPackages
      );
    };
}
