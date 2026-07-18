{nixpkgs}: let
  # ix VMs are x86_64-linux, and the build runs in the builder VM's guest, so
  # the target system is fixed rather than discovered from the host.
  system = "x86_64-linux";

  # Each configuration is a normal NixOS system that differs only by a sentinel
  # package, so a switch onto it moves `/run/current-system` to a new store path
  # you can observe with `command -v <tool>`.
  mkSystem = packages:
    nixpkgs.lib.nixosSystem {
      inherit system;
      modules = [
        ./configuration.nix
        (
          {pkgs, ...}: {
            environment.systemPackages = packages pkgs;
          }
        )
      ];
    };
in {
  nixosConfigurations = {
    # The target VMs. `ix up .#web .#worker .#edge` builds all three closures
    # on the tenant's managed builder and activates each on its own VM.
    web = mkSystem (pkgs: [pkgs.ripgrep]);
    worker = mkSystem (pkgs: [pkgs.jq]);
    edge = mkSystem (pkgs: [pkgs.hello]);
  };
}
