{
  lib,
  pkgs,
  nixCargoUnit,
  rust,
}:
let
  profileArgs =
    profile:
    if profile == "release" then
      [ "--release" ]
    else if profile == "dev" then
      [ ]
    else
      [
        "--profile"
        profile
      ];

  commonArgs = args: {
    inherit (args) src;
    cargoLock = args.cargoLock or (args.src + "/Cargo.lock");
    cargoArgs = args.cargoArgs or [ "--workspace" ];
    cargoTargets =
      let
        targets = args.cargoTargets or [ (args.cargoArgs or [ "--workspace" ]) ];
      in
      if targets == [ ] then
        throw "cargoUnit.buildWorkspace requires at least one cargoTargets entry"
      else
        targets;
    cargoTargetNames = args.cargoTargetNames or null;
    profile = args.profile or "release";
    rustToolchain = args.rustToolchain or rust.defaultRustToolchain;
    nativeBuildInputs = args.nativeBuildInputs or [ ];
    env = args.env or { };
    extraRustcArgs = args.extraRustcArgs or [ ];
    cargoExtraConfig = args.cargoExtraConfig or "";
    vendorDir = args.vendorDir or null;
    vendorSources = args.vendorSources or null;
    # Maps exact Cargo.lock source strings to already-fetched source trees.
    # This keeps private Git dependencies reproducible without requiring
    # sandboxed fetchers to see a developer SSH agent or GitHub credentials.
    sourceOverrides = args.sourceOverrides or { };
    outputHashes = args.outputHashes or { };
    contentAddressed = args.contentAddressed or false;
    policy =
      let
        rawPolicy = args.policy or { };
        rawCargoAudit = rawPolicy.cargoAudit or { };
        resolved = rust.resolvePolicy rawPolicy;
      in
      resolved
      // {
        cargoAudit = resolved.cargoAudit // {
          enable = rawCargoAudit.enable or true;
        };
      };
  };

  workspaceRootFor =
    args:
    args.workspaceRoot or (throw ''
      cargoUnit.buildWorkspace requires workspaceRoot = ./path/to/workspace.
      Use workspaceRoot for the real checkout root that package-shaped sources can be carved from.
      Fetched or patched sources pass workspaceRoot = src.
    '');

  renderCargoArgs =
    args: cargoTarget:
    lib.escapeShellArgs (
      [
        "build"
        "--unit-graph"
        "-Z"
        "unstable-options"
      ]
      ++ profileArgs args.profile
      ++ cargoTarget
      ++ [
        "--frozen"
        "--offline"
      ]
    );

  /**
    Generate Cargo's `--unit-graph` JSON for a vendored Rust workspace.

    This is the first IFD stage used by `buildWorkspace`: Cargo resolves the
    exact rustc units from the caller's locked workspace, with registry and git
    crates supplied by `rustPlatform.importCargoLock`.
  */
  generateUnitGraph =
    rawArgs:
    let
      args = commonArgs rawArgs;
      vendorDir = rust.resolveVendorDir {
        inherit (args)
          cargoLock
          outputHashes
          sourceOverrides
          vendorDir
          ;
      };
    in
    pkgs.runCommand "cargo-unit-graph.json"
      (
        {
          nativeBuildInputs = [
            args.rustToolchain
            pkgs.cacert
            nixCargoUnit
          ]
          ++ args.nativeBuildInputs;
          SSL_CERT_FILE = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";
          # Cargo still gates `--unit-graph` behind `-Z unstable-options`.
          # This helper keeps the input graph generation local to the IFD
          # planner derivation instead of requiring a flake-wide Rust overlay.
          RUSTC_BOOTSTRAP = "1";
        }
        // args.env
      )
      ''
        ${rust.vendorConfigScript {
          inherit vendorDir;
          inherit (args) cargoExtraConfig cargoLock;
        }}

        cd ${args.src}

        pids=
        ${lib.concatMapStringsSep "\n" (
          targetIndex:
          let
            targetArgs = builtins.elemAt args.cargoTargets targetIndex;
          in
          ''
            (
              export CARGO_TARGET_DIR="$TMPDIR/cargo-target-${builtins.toString targetIndex}"
              cargo ${renderCargoArgs args targetArgs} > "$TMPDIR/unit-graph-${builtins.toString targetIndex}.json"
            ) &
            pids="$pids $!"
          ''
        ) (lib.range 0 ((builtins.length args.cargoTargets) - 1))}

        for pid in $pids; do
          wait "$pid"
        done

        nix-cargo-unit merge ${
          lib.concatMapStringsSep " " (
            targetIndex: "$TMPDIR/unit-graph-${builtins.toString targetIndex}.json"
          ) (lib.range 0 ((builtins.length args.cargoTargets) - 1))
        } > "$out"
      '';

  /**
    Render `units.nix` from a Cargo unit graph.

    The result is imported by `buildWorkspace`, so this derivation is the
    second IFD stage. It is separated from `generateUnitGraph` so callers can
    inspect either artifact when debugging graph or renderer behavior.
  */
  generateUnitsNix =
    rawArgs:
    let
      args = commonArgs rawArgs;
      vendorDir = rust.resolveVendorDir {
        inherit (args)
          cargoLock
          outputHashes
          sourceOverrides
          vendorDir
          ;
      };
      unitGraphJson = rawArgs.unitGraphJson or (generateUnitGraph rawArgs);
      toolchainId = builtins.baseNameOf (builtins.toString args.rustToolchain);
      cargoLockForRender = rust.cargoLockFile args.cargoLock;
      renderFlags = [
        "render"
        "--workspace-root"
        (builtins.toString args.src)
        "--vendor-root"
        (builtins.toString vendorDir)
        "--toolchain-id"
        toolchainId
      ]
      ++ lib.optional args.contentAddressed "--content-addressed"
      ++ lib.optional args.policy.denyUnusedCrateDependencies "--deny-unused-crate-dependencies";
    in
    pkgs.runCommand "cargo-units.nix"
      {
        nativeBuildInputs = [ nixCargoUnit ];
        inherit cargoLockForRender;
      }
      ''
        nix-cargo-unit ${lib.escapeShellArgs renderFlags} --cargo-lock "$cargoLockForRender" < ${unitGraphJson} > "$out"
      '';

  /**
    Audit a workspace `Cargo.lock` with `cargo-audit` as a pure Nix check.

    The advisory database is a pinned RustSec checkout by default, and
    `cargo-audit` runs with `--no-fetch --stale` so evaluation and builds do
    not depend on a user Cargo home or network access.
  */
  auditCargoLock =
    rawArgs:
    let
      args = commonArgs rawArgs;
    in
    rust.cargoAuditCheck (
      rawArgs
      // {
        pname = rawArgs.pname or "cargo-unit";
        inherit (args) policy;
      }
    );

  /**
    Build a Rust workspace as one Nix derivation per Cargo rustc unit.

    Each generated unit gets a scoped source input by default. Workspace crates
    receive their own package root, and registry/git crates receive their own
    vendored package directory. A source edit in `crates/api` does not change
    the Nix input for `crates/worker`, `itoa`, or `ryu`; a `Cargo.lock` update
    for one transitive crate leaves unrelated vendored crate derivations alone.
    Git dependency `outputHashes` are keyed by the exact `Cargo.lock` source
    string, including the locked rev, so multi-package git repos share one
    tree hash without losing package identity.
    Pass `workspaceRoot = ./.` for local workspaces so `src` can stay a filtered
    build input while package scopes are carved from the real checkout root.
    Rendering fails when a unit path cannot be tied back to `src` or `vendorDir`.
    Pass `cargoTargets = [ [ "--workspace" ] [ "--workspace" "--tests" ] ]`
    to expose roots from several Cargo executions through one generated graph.
    Include `--benches` or `--bench <name>` to expose `[[bench]]` roots under
    `benchmarks` and `benchmarkPlan`. Tango benches can compare previous and
    next artifacts with `next.compareTangoBenchmarks { baseline = previous; }`,
    where `previous` is another generated workspace or a `benchmarkPlan` path.
    Test graphs also expose `coverageReport` and `makeCoverageReport`; build the
    workspace with `extraRustcArgs = [ "-Cinstrument-coverage" ]` and consume the
    generated `$out/lcov.info`. The selected Rust toolchain must provide matching
    `llvm-cov` and `llvm-profdata`, or callers must pass explicit tool paths to
    `makeCoverageReport`.

    Returns the generated attrset with `sourceAudit`, `units`, `roots`, `checkedRoots`,
    `packages`, `binaries`, `libraries`, `benchmarks`, `coverageReport`, `default`,
    `policyChecks`, plus the intermediate `unitGraphJson`, `unitsNix`, and `vendorDir`
    derivations for inspection.
  */
  buildWorkspace =
    rawArgs:
    let
      args = commonArgs rawArgs;
      workspaceRoot = workspaceRootFor rawArgs;
      vendorDir = rust.resolveVendorDir {
        inherit (args)
          cargoLock
          outputHashes
          sourceOverrides
          vendorDir
          ;
      };
      vendorSources = rust.resolveVendorSources {
        inherit (args)
          cargoLock
          outputHashes
          sourceOverrides
          vendorSources
          ;
      };
      unitGraphJson = generateUnitGraph (rawArgs // { inherit vendorDir; });
      unitsNix = generateUnitsNix (
        rawArgs
        // {
          inherit unitGraphJson vendorDir;
        }
      );
      units = import unitsNix {
        inherit pkgs vendorDir vendorSources;
        inherit (args)
          src
          extraRustcArgs
          rustToolchain
          ;
        inherit workspaceRoot;
        extraNativeBuildInputs = args.nativeBuildInputs ++ rust.nativeBuildInputsForPolicy args.policy;
        extraEnv = args.env;
        extraRustcArgsForPlatform = rust.rustcArgsForPolicyForPlatform args.policy;
        extraPolicyChecks = rust.policyChecksFor (
          rawArgs
          // {
            inherit vendorDir;
            inherit (args) policy;
          }
        );
      };
      targetSetNames =
        if args.cargoTargetNames == null then
          map (index: builtins.toString index) (lib.range 0 ((builtins.length args.cargoTargets) - 1))
        else if builtins.length args.cargoTargetNames == builtins.length args.cargoTargets then
          args.cargoTargetNames
        else
          throw "cargoUnit.buildWorkspace requires cargoTargetNames to match cargoTargets length";
      namedTargetSets = lib.listToAttrs (
        lib.imap1 (
          targetIndex: targetName:
          lib.nameValuePair targetName (builtins.elemAt units.targetSets (targetIndex - 1))
        ) targetSetNames
      );
    in
    units
    // {
      inherit unitGraphJson unitsNix vendorDir;
      targetSets = namedTargetSets;
      inherit (args) policy;
    };

  /**
    Select one binary target from a generated workspace graph.
  */
  buildBinary =
    {
      binary,
      cargoArgs ? [ ],
      ...
    }@args:
    let
      workspace = buildWorkspace (builtins.removeAttrs args [ "binary" ]);
    in
    workspace.binaries.${binary} or workspace.default;

  /**
    Select several binary targets from one workspace unit graph.

    Use `cargoTargets` on `buildWorkspace` when the same import should expose
    roots from several Cargo executions, such as build and test graphs.
  */
  buildBinaries =
    {
      binaries,
      cargoArgs ? [ ],
      ...
    }@args:
    let
      workspace = buildWorkspace (builtins.removeAttrs args [ "binaries" ]);
    in
    lib.genAttrs binaries (binary: workspace.binaries.${binary} or workspace.default);
in
{
  inherit
    buildBinary
    buildBinaries
    buildWorkspace
    auditCargoLock
    generateUnitGraph
    generateUnitsNix
    ;
}
