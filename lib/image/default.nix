{
  lib,
  nixpkgs,
  rust-overlay,
  paths,
  system,
  home-manager,
  overlays,
  ixSpecialArgs,
  moduleList,
  writeNushellApplication,
  packageSetFor,
  # The index flake's own `self`, for the guest `index` registry pin (see the
  # `nix.registry.index` module below). `null` when `lib` is imported without a
  # flake; the pin is then omitted.
  self ? null,
}: let
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

  Unfree packages enter images only by explicit name here, never by
  flipping `allowUnfree`. Every image shares this one instance via
  `nixpkgs.pkgs`, and the nixpkgs module then ignores a per-image
  `nixpkgs.config` (setting one even fails an assertion), so an image's
  unfree exception has to be added to this predicate, not to the image.
    - `yourkit-java`: the opt-in `ix.languages.java.yourkit` profiler agent
      an operator turns on for performance work.
      Refs: https://www.yourkit.com/docs/java/help/agent.jsp
    - `claude-code`: Anthropic's agent CLI imported by the dev base module;
      ships under commercial terms.
  The predicate keeps every other unfree (Oracle JDK, Adobe runtimes,
  NVIDIA blobs) failing at eval until the platform allows it explicitly.
  */
  imagePkgs = import nixpkgs {
    inherit system overlays;
    config = {
      allowUnfreePredicate = pkg:
        builtins.elem (lib.getName pkg) [
          "yourkit-java"
          "claude-code"
        ];
    };
  };

  /**
  Locked flake-input sources baked into the base image but NOT reachable
  through a `nix.registry.*` pin, so `system.extraDependencies` roots them
  into the system closure (and `includeNixDB` in oci-layer.nix registers
  everything in that closure as valid). Single source of truth: the module
  below sets `system.extraDependencies` to this list, and the
  `base-image-nix-db` check (tests/default.nix) reads it back off the
  evaluated `system.extraDependencies` to assert each path ships valid — the
  registry-derived projection there cannot catch these, precisely because
  they are not registry pins.

  Measurement-driven exception to the "don't bake the flake inputs" hold
  (index #1748/#1815): on the current base image the FIRST `nix run index#jq`
  in a fresh ix VM took 2m44.8s, its log showing `unpacking
  'github:oxalica/rust-overlay/107c334f...' into the Git cache` — evaluating
  index's flake under `nix run` forces the `rust-overlay` input source, which
  the image does not ship, so nix fetches and unpacks it through VCFS. Baking
  it drops that to the warm ~2.6s. rust-overlay is ~19M; home-manager and
  hermes-agent stay unbaked until a measurement justifies each.

  `.outPath` is the ORIGINAL `-source` store path with string context, so it
  roots into the closure once (no duplicate copy — the #1748 trap); the path
  must be the LOCKED input so it matches what index's `flake.lock` narHash
  resolves to during in-guest eval.
  */
  extraBakedSources = [rust-overlay.outPath];

  /**
  Run the platform config, OCI packaging, base profile, the full module
  registry, and the caller's `modules` through `lib.nixosSystem`, then
  return the evaluated `config`. This is the evaluation path every
  image build and every eval test goes through, so a test exercising it
  catches the same regressions a real build would.

  Arguments:
  - `modules`: list of additional modules layered on top of the base.
  */
  evalImageConfig = {modules ? []}:
    (lib.nixosSystem {
      inherit system;
      specialArgs.ix = ixSpecialArgs;
      modules =
        [
          {nixpkgs.pkgs = imagePkgs;}
          {
            # Pin the system flake registry so `nix shell nixpkgs#foo` resolves
            # against the nixpkgs bundled in the image instead of fetching a
            # fresh tarball from GitHub on every invocation (~40 MB download,
            # 100k files extracted, 20+ minutes on VCFS).
            #
            # `narHash` locks the pin. Without it nix treats the `path:` input
            # as mutable and re-hashes AND re-copies the whole ~45k-file tree
            # into /nix/store on every eval; through the guest's virtiofs/VCFS
            # store that is ~3 minutes per `nix eval`/`nix run`, ~1 s locked
            # (measured in an `ix new` VM, 2026-07-02). `outPath` (a string)
            # rather than the path value also keeps `toJSON` from copying a
            # duplicate nixpkgs tree into the image closure. Lives here, not
            # platform.nix, because only this scope sees the flake input's
            # `narHash`.
            nix.registry.nixpkgs.to = {
              type = "path";
              path = nixpkgs.outPath;
              inherit (nixpkgs) narHash;
            };
          }
          {
            # Root the extra baked sources (see `extraBakedSources` above) into
            # the system closure so `includeNixDB` registers them as valid and
            # an in-guest `nix run index#...` finds rust-overlay already present
            # instead of unpacking it through VCFS (measured 2m44.8s -> ~2.6s).
            system.extraDependencies = extraBakedSources;
          }
        ]
        ++ lib.optional (self != null) {
          # Same treatment for the `index` flake itself, so an in-guest
          # `nix run index#<pkg>` (and any flake declaring `index` as an input
          # at this locked rev) resolves against the source baked in the image
          # instead of fetching it from GitHub. The base image's nix store DB
          # (`includeNixDB`, oci-layer.nix) registers this `-source` path as
          # valid — nix then treats the locked, narHash-matched reference as
          # already present and never re-fetches or re-ingests it, the same
          # property nixpkgs got in ix#6043/#1748/#1749/#1815.
          #
          # `self.outPath` is the ORIGINAL `-source` path and carries string
          # context, so it roots into the image closure once (no duplicate
          # copy — the #1748 trap). `self.narHash` locks the pin. Only this
          # flake scope sees `self`, so it is plumbed down from `flake.nix`.
          nix.registry.index.to = {
            type = "path";
            path = self.outPath;
            inherit (self) narHash;
          };
        }
        ++ [
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

  # Shared bootstrap OCI reference used to materialize missing fleet nodes.
  # The archive is built and published outside the default flake checks.
  bootstrapImage = {
    name = "ix/test-cluster-bootstrap";
    tag = "zstd-tools-2026-05-12";
  };

  /**
  Build a fleet plan helper for a given host system. Returns a function
  that takes a fleet spec and produces the plan/commands tooling consumes.
  `mkFleet` is the default-system shortcut.
  */
  mkFleetFor = hostSystem: let
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
      ixFleet = (packageSetFor hostPkgs).ix-fleet;
    };

  mkFleet = mkFleetFor system;

  # Dev-fleet layer over `mkFleet` (RFC 0007): consumes the forkable `ix.nix`
  # spec. Curried like `mkFleetFor` so example/flake eval can target a host
  # system.
  inherit
    (import ./dev.nix {
      inherit
        lib
        paths
        mkFleetFor
        evalImageConfig
        ;
    })
    mkDevFor
    ;
  mkDev = mkDevFor system;

  # Non-NixOS OCI images are built standalone (no `nixosSystem`), so they need a
  # plain package set carrying the ix overlay for `oci-image-builder`. Reusing
  # the same overlays the image evaluation applies keeps both builders on one
  # toolchain.
  hostPkgs = import nixpkgs {
    inherit system overlays;
    config = {};
  };

  inherit
    (import ./non-nix-oci.nix {
      inherit lib;
      pkgs = hostPkgs;
    })
    mkNonNixImage
    ;
in {
  inherit
    evalImageConfig
    mkImage
    mkNonNixImage
    bootstrapImage
    mkFleetFor
    mkFleet
    mkDevFor
    mkDev
    ;
}
