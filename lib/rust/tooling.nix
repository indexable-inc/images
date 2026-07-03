{
  lib,
  packagePath,
  languages,
  writePythonApplication,
  rustWorkspaceFor,
  clippy-fork,
  repoRoot,
  lists,
  # Shared pins reader, threaded through to policy.nix (see its arg doc).
  pins,
}: let
  inherit (builtins) toString;

  repoRustToolchainFile = lib.importTOML (repoRoot + "/rust-toolchain.toml");
  repoRustChannel = repoRustToolchainFile.toolchain.channel;
  repoRustNightlyDate = assert lib.assertMsg (lib.hasPrefix "nightly-" repoRustChannel)
  "rust-toolchain.toml must pin a nightly channel for repo-owned Rust packages";
    lib.removePrefix "nightly-" repoRustChannel;
  rustFor = pkgs:
    import ./build.nix {
      inherit
        lib
        pkgs
        lists
        pins
        ;
      # llm-clippy bootstraps before cargoUnit / rustWorkspace exist, so the
      # `ix` closure it receives carries only `buildRustPackage`.
      # `buildIxRustTool` adds the richer surface for packages that need it.
      clippyPackage = pkgs.callPackage (packagePath "llm-clippy") {
        ix = {
          inherit buildRustPackage pkgs;
        };
        inherit clippy-fork;
      };
      rustToolchain = languages.rust.toolchain pkgs {
        channel = "nightly";
        version = repoRustNightlyDate;
      };
      writePythonApplication = writePythonApplication pkgs;
    };
  # Build a repo-owned Rust tool while keeping nix-cargo-unit itself on the
  # pre-cargo-unit bootstrap path.
  # Returns the policy-unchecked variant when present, so generators that
  # only need the binary do not drag the policy-check graph into their closure.
  buildIxRustTool = hostPkgs: path: let
    usesCargoUnit = toString path != toString (packagePath "nix-cargo-unit");

    hostRustWorkspace = rustWorkspaceFor hostPkgs;

    checked = hostPkgs.callPackage path {
      ix =
        {
          inherit buildRustPackage;
          pkgs = hostPkgs;
          rustWorkspace = hostRustWorkspace;
        }
        // lib.optionalAttrs usesCargoUnit {
          cargoUnit = cargoUnitFor hostPkgs;
        };
    };

    hasUnchecked = checked.passthru ? unchecked;
  in
    # Repo Rust tools built through `ix.buildRustPackage` expose the
    # policy-unchecked binary as `passthru.unchecked`; prefer it so a generator
    # that only needs the binary doesn't pull the policy-check graph into its
    # closure. A package built another way has no such variant, so use it as-is.
    if hasUnchecked
    then let
      unchecked = checked.passthru.unchecked;
      meta = (unchecked.meta or {}) // (checked.meta or {});
    in
      unchecked // {inherit meta;}
    else checked;

  cargoUnitFor = pkgs:
    import ./cargo-unit.nix {
      inherit lib pkgs;
      rust = rustFor pkgs;
      nixCargoUnit = buildIxRustTool pkgs (packagePath "nix-cargo-unit");
    };
  /**
  Build a repo-owned Rust package with the shared Rust policy.

  Wraps `rustPlatform.buildRustPackage`, enables parallel test execution by
  default, and attaches the repo's `llm-clippy` and unused-dependency checks
  as `passthru.tests` plus policy dependencies of the returned package.
  */
  buildRustPackage = pkgs: (rustFor pkgs).buildPackage;
in {
  inherit
    buildIxRustTool
    cargoUnitFor
    buildRustPackage
    ;
}
