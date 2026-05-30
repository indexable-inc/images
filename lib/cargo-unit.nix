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
    testRunPrelude = args.testRunPrelude or "";
    testArgsByPackage = args.testArgsByPackage or { };
    packageTestInputs = args.packageTestInputs or { };
    packageTestEnv = args.packageTestEnv or { };
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

  # Cargo only emits `[lints.clippy]` into the unit graph's `lint_rustflags`
  # when invoked as `cargo clippy`, not `cargo build`. Our unit graph is built
  # with `cargo build --unit-graph`, so per-unit clippy never sees the
  # workspace lint policy unless we resolve it ourselves. Parse the workspace
  # manifest and emit the equivalent `-D|-W|-A clippy::<lint>` flags.
  #
  # Per-package overrides (a package with its own `[lints.clippy]` and
  # `workspace = false`) are not yet honored; most workspaces in practice use
  # `[lints] workspace = true` per crate, which inherits the workspace table.
  #
  # `clippy::cargo` group lints and the individual members of that group
  # invoke the `cargo` binary to read workspace metadata. Per-unit clippy
  # runs in a sandboxed build directory without a discoverable Cargo.toml
  # (the unit's source closure is package-shaped, not workspace-shaped), so
  # those lints error out with "could not find Cargo.toml". They only make
  # sense at workspace scope; skip them here and leave a workspace-level
  # cargo-clippy check as the future home for that subset.
  cargoGroupClippyLints = [
    "cargo"
    "cargo_common_metadata"
    "multiple_crate_versions"
    "negative_feature_names"
    "redundant_feature_names"
    "wildcard_dependencies"
  ];
  clippyLintFlagsFromManifest =
    manifestPath:
    let
      manifest = lib.importTOML manifestPath;
      raw = manifest.workspace.lints.clippy or manifest.lints.clippy or { };
      filtered = builtins.removeAttrs raw cargoGroupClippyLints;
      entryFor =
        name: value:
        if builtins.isString value then
          {
            inherit name;
            level = value;
            priority = 0;
          }
        else
          {
            inherit name;
            inherit (value) level;
            priority = value.priority or 0;
          };
      entries = lib.mapAttrsToList entryFor filtered;
      # Cargo applies args in ascending-priority order so higher-priority lints
      # appear later on the command line and win as overrides. Mirror that here
      # so a per-lint allow can override a group-wide deny.
      sorted = lib.sort (left: right: left.priority < right.priority) entries;
      flagFor =
        level:
        if level == "deny" || level == "forbid" then
          "-D"
        else if level == "warn" then
          "-W"
        else if level == "allow" then
          "-A"
        else
          throw "cargoUnit: unknown clippy lint level '${level}' in ${manifestPath}";
    in
    lib.concatMap (entry: [
      (flagFor entry.level)
      "clippy::${entry.name}"
    ]) sorted;

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
      ++ lib.optional args.policy.denyUnusedCrateDependencies "--deny-unused-crate-dependencies"
      ++ lib.optional args.policy.denyPanics "--deny-panics";
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
      perUnitClippyEnabled = args.policy.clippy.enable;
      # Per-unit clippy runs `clippy-driver` directly on each non-external
      # unit. Suppress the legacy workspace-level `cargoClippy` derivation in
      # that mode so the same lints don't run twice and so a single source
      # edit doesn't invalidate every other crate's clippy.
      extraPolicyChecksFromRust = rust.policyChecksFor (
        rawArgs
        // {
          inherit vendorDir;
          policy =
            args.policy
            // lib.optionalAttrs perUnitClippyEnabled {
              clippy = args.policy.clippy // {
                enable = false;
              };
            };
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
        # Scanner for the opt-in panic-freedom policy. The rendered check
        # asserts this is non-null when `policy.denyPanics` is set.
        cargoUnit = nixCargoUnit;
        extraNativeBuildInputs = args.nativeBuildInputs ++ rust.nativeBuildInputsForPolicy args.policy;
        # `clippy-driver` ships in the clippy package; `rustToolchain` only
        # guarantees rustc + cargo. Adding the resolved clippy package keeps
        # version drift impossible because the toolchain pins the rustc that
        # `clippy-driver` links against.
        extraClippyNativeBuildInputs = lib.optional perUnitClippyEnabled args.policy.clippy.package;
        extraEnv = args.env;
        inherit (args)
          testRunPrelude
          testArgsByPackage
          packageTestInputs
          packageTestEnv
          ;
        extraRustcArgsForPlatform = rust.rustcArgsForPolicyForPlatform args.policy;
        # Manifest-derived flags come first so per-call `policy.clippy`
        # entries land later in argv and can override them. Cargo's
        # `[lints.clippy]` resolution is the load-bearing source for most
        # workspaces; `policy.clippy.deniedLints` stays as an escape hatch
        # for callers without a Cargo.toml policy.
        extraClippyLintArgs =
          clippyLintFlagsFromManifest (args.src + "/Cargo.toml") ++ rust.clippyLintArgs args.policy;
        clippyEnabled = perUnitClippyEnabled;
        extraPolicyChecks = extraPolicyChecksFromRust;
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
    Pick a binary out of a pre-built `buildWorkspace` plus its test
    derivations, ready for `passthru.tests` consumption.

    `testTargets` and `doctestTargets` default to every generated target owned
    by `packageName`. Each discovered test case becomes its own derivation by
    default; `<target>-all` remains available for callers that need the full
    harness as a single compatibility check.

    Use this when the caller has one shared workspace (`ix.rustWorkspace.units`)
    so all repo-owned crates ride the same unit graph. Use `buildBinary` when
    a crate needs its own workspace (different policy, fetched source, etc).
  */
  selectBinaryWithTests =
    workspace:
    {
      binary,
      packageName ? binary,
      testTargets ? null,
      doctestTargets ? null,
      includeTestCases ? true,
      meta ? { },
      passthru ? { },
    }:
    selectRootWithTests workspace {
      rootDrv = workspace.binaries.${binary} or workspace.default;
      inherit
        packageName
        testTargets
        doctestTargets
        includeTestCases
        meta
        passthru
        ;
      defaultTestTargets = [ binary ];
    };

  /**
    Pick a library target from a pre-built `buildWorkspace` plus its test and
    doctest derivations, ready for `passthru.tests` consumption.

    The library version of `selectBinaryWithTests`, for crates that ship a
    `lib` target rather than a binary. `library` is the crate's library unit
    key (Cargo's underscored name, e.g. `ix_vt`); `packageName` is the Cargo
    package name used to look up test targets (e.g. `ix-vt`).
  */
  selectLibraryWithTests =
    workspace:
    {
      library,
      packageName,
      testTargets ? null,
      doctestTargets ? null,
      includeTestCases ? true,
      meta ? { },
      passthru ? { },
    }:
    selectRootWithTests workspace {
      rootDrv =
        workspace.libraries.${library}
          or (throw "selectLibraryWithTests: no library `${library}` in workspace; available: ${
            lib.concatStringsSep ", " (builtins.attrNames (workspace.libraries or { }))
          }");
      inherit
        packageName
        testTargets
        doctestTargets
        includeTestCases
        meta
        passthru
        ;
      defaultTestTargets = [ packageName ];
    };

  # Shared core for `selectBinaryWithTests` / `selectLibraryWithTests`: take a
  # selected root derivation and assemble its `passthru.tests` from the shared
  # workspace's test/doctest targets and policy checks.
  selectRootWithTests =
    workspace:
    {
      rootDrv,
      packageName,
      defaultTestTargets,
      testTargets ? null,
      doctestTargets ? null,
      includeTestCases ? true,
      meta ? { },
      passthru ? { },
    }:
    let
      uncheckedRoot = rootDrv.passthru.unchecked or rootDrv;
      namesForPackage =
        attrName: fallback:
        if builtins.hasAttr attrName workspace && builtins.hasAttr packageName workspace.${attrName} then
          workspace.${attrName}.${packageName}
        else
          fallback;
      selectedTestTargets =
        if testTargets == null then
          namesForPackage "testTargetNamesByPackage" defaultTestTargets
        else
          testTargets;
      selectedDoctestTargets =
        if doctestTargets == null then
          namesForPackage "doctestTargetNamesByPackage" [ ]
        else
          doctestTargets;
      flattenAllTargets =
        prefix: targetNames: targets:
        lib.mapAttrs' (targetName: target: lib.nameValuePair "${prefix}${targetName}-all" target.all) (
          lib.getAttrs (builtins.filter (name: targets ? ${name}) targetNames) targets
        );
      flattenCaseTargets =
        prefix: targetNames: targets:
        lib.concatMapAttrs (
          targetName: target:
          lib.mapAttrs' (
            case: drv:
            lib.nameValuePair "${prefix}${targetName}-${lib.replaceStrings [ "::" ] [ "-" ] case}" drv
          ) (target.cases or { })
        ) (lib.getAttrs (builtins.filter (name: targets ? ${name}) targetNames) targets);
      policyChecks = workspace.policyChecks or { };
      testCases =
        flattenCaseTargets "" selectedTestTargets (workspace.tests or { })
        // flattenCaseTargets "doctest-" selectedDoctestTargets (workspace.doctests or { });
      tests = {
        package = uncheckedRoot;
      }
      // flattenAllTargets "" selectedTestTargets (workspace.tests or { })
      // flattenAllTargets "doctest-" selectedDoctestTargets (workspace.doctests or { })
      // lib.optionalAttrs includeTestCases testCases;
    in
    rootDrv
    // {
      meta = (rootDrv.meta or { }) // meta;
      passthru =
        (rootDrv.passthru or { })
        // passthru
        // {
          tests = (rootDrv.passthru.tests or { }) // policyChecks // (passthru.tests or { }) // tests;
          inherit policyChecks;
          inherit (workspace) policy;
        };
    };

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

  /**
    Materialize a Cargo vendor directory from a `Cargo.lock` without building
    the workspace unit graph. Callers that aren't going through
    `buildWorkspace` (e.g. an `overrideAttrs` of a foreign Rust derivation, or
    a non-workspace tool fetched as a single crate) can reuse the same
    static.crates.io fetcher and git-source plumbing that `buildWorkspace`
    uses internally.

    Arguments:
    - `cargoLock`: path to the `Cargo.lock` to vendor.
    - `outputHashes`: attrset keyed by the exact `Cargo.lock` git source
      string (e.g. `"git+https://github.com/owner/repo#rev"`); value is the
      sha256 of the resolved git tree.
    - `sourceOverrides`: optional attrset mapping `Cargo.lock` source strings
      to pre-fetched source trees, used when a private git dependency cannot
      be fetched from inside the build sandbox.
    - `vendorDir`: optional pre-built vendor directory that short-circuits
      resolution. Mirrors `buildWorkspace`'s `vendorDir` arg.

    Returns a `pkgs.linkFarm` of `<name>-<version>` -> source tree, the same
    shape `buildWorkspace` materializes.
  */
  vendorDir =
    args:
    rust.resolveVendorDir {
      inherit (args) cargoLock;
      outputHashes = args.outputHashes or { };
      sourceOverrides = args.sourceOverrides or { };
      vendorDir = args.vendorDir or null;
    };

  /**
    Materialize the per-package source attrset used by `vendorDir`. Useful for
    callers that need to address individual vendored crates rather than the
    aggregate link farm. See `vendorDir` for the shared argument shape.
  */
  vendorSources =
    args:
    rust.resolveVendorSources {
      inherit (args) cargoLock;
      outputHashes = args.outputHashes or { };
      sourceOverrides = args.sourceOverrides or { };
      vendorSources = args.vendorSources or null;
    };
in
{
  inherit
    buildBinary
    buildBinaries
    buildWorkspace
    selectBinaryWithTests
    selectLibraryWithTests
    auditCargoLock
    generateUnitGraph
    generateUnitsNix
    vendorDir
    vendorSources
    ;
}
