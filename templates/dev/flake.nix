{
  description = "My ix dev environment (RFC 0007)";

  inputs.index.url = "github:indexable-inc/index";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    {
      self,
      index,
      nixpkgs,
      ...
    }:
    let
      # Systems you build VMs from. ix VM closures are linux; add darwin only if
      # you evaluate from a Mac.
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forEach = f: nixpkgs.lib.genAttrs systems f;
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

      # Each node's NixOS system under its bare name, so `ix up .#<node>` and the
      # native multi-VM switch (`ix up .#a .#b --build-vm <builder>`) resolve it.
      # ix VM closures are x86_64-linux, so the configs come from that builder.
      inherit
        (index.lib.mkDevFor "x86_64-linux" {
          module = ./dev.nix;
          src = self;
        })
        nixosConfigurations
        ;
    };
}
