{
  lib,
  packagePath,
  languages,
  writePythonApplication,
  rustWorkspaceFor,
}:
let
  rustNightlyToolchainFor =
    pkgs:
    languages.rust.toolchain pkgs {
      channel = "nightly";
      version = languages.rust.defaultNightlyDate;
    };
  rustNightlyClippyToolchainFor =
    pkgs:
    languages.rust.toolchain pkgs {
      channel = "nightly";
      version = languages.rust.defaultNightlyDate;
      components = [
        "cargo"
        "llvm-tools"
        "rust-src"
        "rust-std"
        "rustc"
        "rustc-dev"
        "rustfmt"
      ];
    };
  llmClippyFor =
    pkgs:
    pkgs.callPackage (packagePath "llm-clippy") {
      rustToolchain = rustNightlyClippyToolchainFor pkgs;
    };
  rustFor =
    pkgs:
    import ./rust.nix {
      inherit lib pkgs;
      clippyPackage = llmClippyFor pkgs;
      rustToolchain = rustNightlyToolchainFor pkgs;
      writePythonApplication = writePythonApplication pkgs;
    };
  # Build a repo-owned Rust tool while keeping nix-cargo-unit itself on the
  # pre-cargo-unit bootstrap path.
  # Returns the policy-unchecked variant when present, so generators that
  # only need the binary do not drag the policy-check graph into their closure.
  buildIxRustTool =
    hostPkgs: path:
    let
      usesCargoUnit = builtins.toString path != builtins.toString (packagePath "nix-cargo-unit");
      hostRustWorkspace = rustWorkspaceFor hostPkgs;
      checked = hostPkgs.callPackage path {
        pkgs = hostPkgs;
        ix = {
          buildRustPackage = pkgs: (rustFor pkgs).buildPackage;
          rustWorkspace = hostRustWorkspace;
        }
        // lib.optionalAttrs usesCargoUnit {
          cargoUnit = cargoUnitFor hostPkgs;
        };
      };
      unchecked = checked.passthru.unchecked or null;
    in
    if unchecked == null then
      checked
    else
      unchecked
      // {
        meta = (unchecked.meta or { }) // (checked.meta or { });
      };
  cargoUnitFor =
    pkgs:
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
in
{
  inherit
    rustNightlyToolchainFor
    rustNightlyClippyToolchainFor
    llmClippyFor
    rustFor
    buildIxRustTool
    cargoUnitFor
    buildRustPackage
    ;
}
