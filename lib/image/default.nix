{
  lib,
  nixpkgs,
  paths,
  system,
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
    One nixpkgs instance shared by every image evaluation. `lib.nixosSystem`
    otherwise instantiates a fresh nixpkgs PER node, and a consumer that
    evaluates many images in one evaluation (the ix fleet's regional-status
    canary evaluates every example fleet x 2 regions inside one NixOS host
    config) pays that instantiation 20-30 times over: 656M thunks / 5.5min
    of nix CPU for one host, and an OOM-killed deploy under nox (ix
    ENG-2728/ENG-2729). The parameters fold in exactly what the per-node
    modules used to set: the repo overlay (previously a `nixpkgs.overlays`
    module) and the platform config from `platform.nix`.

    YourKit is the only unfree we currently allow into images, and only
    because `ix.languages.java.yourkit` is an opt-in profiler agent that
    an operator turns on for performance work. The predicate keeps every
    other unfree (Oracle JDK, Adobe runtimes, NVIDIA blobs) failing at
    eval until the platform decides to allow it explicitly.
    Refs: https://www.yourkit.com/docs/java/help/agent.jsp
  */
  imagePkgs = import nixpkgs {
    inherit system overlays;
    config.allowUnfreePredicate = pkg: builtins.elem (lib.getName pkg) [ "yourkit-java" ];
  };

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
        { nixpkgs.pkgs = imagePkgs; }
        ./platform.nix
        ./oci-layer.nix
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
      hostPkgs = nixpkgs.legacyPackages."${hostSystem}";
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

  # Non-NixOS OCI images are built standalone (no `nixosSystem`), so they need a
  # plain package set carrying the ix overlay for `oci-image-builder`. Reusing
  # the same overlays the image evaluation applies keeps both builders on one
  # toolchain.
  hostPkgs = import nixpkgs {
    inherit system overlays;
  };

  inherit
    (import ./non-nix-oci.nix {
      inherit lib;
      pkgs = hostPkgs;
    })
    mkNonNixImage
    ;
in
{
  inherit
    evalImageConfig
    mkImage
    mkNonNixImage
    bootstrapImage
    mkFleetFor
    mkFleet
    ;
}
