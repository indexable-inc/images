{
  lib,
  nixpkgs,
  paths,
  system,
  determinate,
  home-manager,
  overlays,
  ixSpecialArgs,
  moduleList,
  writeNushellApplication,
  secrets,
  packageSetFor,
}:
let
  /**
    Run the platform config, OCI packaging, base profile, the full module
    registry, and the caller's `modules` through `lib.nixosSystem`, then
    return the evaluated `config`. This is the evaluation path every
    image build and every eval test goes through, so a test exercising it
    catches the same regressions a real build would.

    Arguments:
    - `modules`: list of additional modules layered on top of the base.
  */
  evalImageConfig =
    {
      modules ? [ ],
    }:
    (lib.nixosSystem {
      inherit system;
      specialArgs.ix = ixSpecialArgs;
      modules = [
        { nixpkgs.overlays = overlays; }
        ./ix-platform.nix
        ./ix-oci-layer.nix
        # Determinate Nix replaces the in-VM nix package and daemon. The
        # module sets nix.package, configures determinate-nixd as a systemd
        # service, and seeds nix.settings defaults. Compatible with
        # boot.isContainer = true since the daemon runs under our PID 1.
        determinate.nixosModules.default
        # Home Manager as a NixOS module. Per-tool XDG config (Nushell,
        # atuin, zoxide, starship, ...) is configured under
        # `home-manager.users.root` in the base profile; this module
        # exposes the option set and shares the system pkgs.
        home-manager.nixosModules.home-manager
        {
          home-manager = {
            useGlobalPkgs = true;
            useUserPackages = true;
            # Activation renames existing user files with this extension
            # instead of failing, so an operator who hand-edited a config
            # sees the conflict rather than losing the file.
            backupFileExtension = "hm-backup";
          };
        }
      ]
      ++ moduleList
      ++ modules;
    }).config;

  /**
    Build one self-contained OCI archive from a list of NixOS modules.

    Each image is independent: ix does not stack images at runtime, it
    runs one. Returns the OCI-archive derivation; pass it to
    `ix image push` or use it as a `packages.<system>.<name>` output.
  */
  mkImage = args: (evalImageConfig args).ix.build.ociImage;

  # Shared NixOS bootstrap image used to materialize missing fleet nodes.
  # Reads the canonical name/tag from the image module so the fleet default
  # and the image being published can't drift.
  bootstrapImage =
    (evalImageConfig {
      modules = [ (paths.images + "/system/test-cluster-bootstrap") ];
    }).ix.image;

  /**
    Build a fleet plan helper for a given host system. Returns a function
    that takes a fleet spec and produces the plan/commands tooling consumes.
    `mkFleet` is the default-system shortcut.
  */
  mkFleetFor =
    hostSystem:
    let
      hostPkgs = nixpkgs.legacyPackages.${hostSystem};
    in
    import ./fleet.nix {
      inherit
        lib
        evalImageConfig
        writeNushellApplication
        bootstrapImage
        ;
      pkgs = hostPkgs;
      secretsLib = secrets;
      ixFleet = (packageSetFor hostPkgs).ix-fleet;
    };

  mkFleet = mkFleetFor system;
in
{
  inherit
    evalImageConfig
    mkImage
    bootstrapImage
    mkFleetFor
    mkFleet
    ;
}
