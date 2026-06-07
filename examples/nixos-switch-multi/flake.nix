{
  description = "ix up multi-VM switch: one build VM, several NixOS VMs switched in one command";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";

  outputs =
    { nixpkgs, ... }:
    let
      # ix VMs are x86_64-linux, and the build runs in the builder VM's guest, so
      # the target system is fixed rather than discovered from the host.
      system = "x86_64-linux";

      # Each configuration is a normal NixOS system that differs only by a sentinel
      # package, so a switch onto it moves `/run/current-system` to a new store
      # path you can observe with `command -v <tool>`.
      mkSystem =
        packages:
        nixpkgs.lib.nixosSystem {
          inherit system;
          modules = [
            ./configuration.nix
            (
              { pkgs, ... }:
              {
                environment.systemPackages = packages pkgs;
              }
            )
          ];
        };
    in
    {
      nixosConfigurations = {
        # The shared build VM. It only needs Nix (every NixOS system has the
        # daemon), so it carries no sentinel; `ix up .#builder` brings it up once.
        builder = mkSystem (_: [ ]);

        # The target VMs. `ix up .#web .#worker .#edge --build-vm builder` builds
        # all three closures on `builder` and activates each on its own VM.
        web = mkSystem (pkgs: [ pkgs.ripgrep ]);
        worker = mkSystem (pkgs: [ pkgs.jq ]);
        edge = mkSystem (pkgs: [ pkgs.hello ]);
      };
    };
}
