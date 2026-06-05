{
  lib,
  pkgs,
  nixCargoUnit,
  rust,
}:
let
  # The toolchain id baked into every unit hash for the default toolchain
  # (`generateUnitsNix` computes the same `baseNameOf (toString rustToolchain)`,
  # lib/rust/cargo-unit.nix). Exposed so callers of `mkPrebuiltLibraryUnit` can
  # record and assert the id a prebuilt rlib was compiled with without
  # reconstructing it by hand.
  defaultToolchainId = builtins.baseNameOf (builtins.toString rust.defaultRustToolchain);

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
    # Optional cross-compile triple (e.g. "aarch64-apple-darwin"). When set,
    # `--target` is threaded into the unit-graph generation; nix-cargo-unit's
    # renderer then emits `--target` per unit and reads the matching
    # `CARGO_TARGET_<T>_LINKER` from `env`. `null` keeps the host-native build.
    target = args.target or null;
    # Caller hook composed with the policy (mold) per-platform args. Receives
    # each unit's platform string and returns extra rustc args. Used to thread
    # the Apple framework search path for cross Darwin units only.
    extraRustcArgsForPlatform = args.extraRustcArgsForPlatform or (_platform: [ ]);
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
    # Additive seam for injecting prebuilt units (rlib+rmeta) that were not built
    # from source. `extraUnits` is keyed by the unit key
    # (`"<name>-<version>-<hash>"`) and merged over the generated `units` set;
    # `extraLibraries` is keyed by the underscored library name and merged over
    # `libraries`. Both default to `{}`, so an unconfigured workspace is
    # byte-identical to one built before the seam existed. See
    # `mkPrebuiltLibraryUnit` for the producer.
    extraUnits = args.extraUnits or { };
    extraLibraries = args.extraLibraries or { };
    # Maps exact Cargo.lock source strings to already-fetched source trees.
    # This keeps private Git dependencies reproducible without requiring
    # sandboxed fetchers to see a developer SSH agent or GitHub credentials.
    sourceOverrides = args.sourceOverrides or { };
    outputHashes = args.outputHashes or { };
    # Default ON: workspace units emit floating content-addressed outputs so an
    # output-invariant rebuild of a low-level crate early-cuts off instead of
    # cascading through its whole reverse-dependency closure. Consumers may still
    # pass `contentAddressed = false` to opt out. (ca-derivations must be enabled
    # in the daemon; ix CI enables it.)
    contentAddressed = args.contentAddressed or true;
    policy = rust.resolvePolicy (args.policy or { });
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
      ++ lib.optionals (args.target != null) [
        "--target"
        args.target
      ]
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
      # Import the rendered units.nix with a given prebuilt-injection seam. The
      # generated (pre-seam) set is obtained by importing with empty seam args,
      # so the injection guards below can compare against the real generated keys
      # without a second IFD (the import is memoized; only the function call
      # differs). See mkPrebuiltLibraryUnit.
      importUnits =
        seam:
        import unitsNix (
          {
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
            extraRustcArgsForPlatform =
              platform:
              rust.rustcArgsForPolicyForPlatform args.policy platform ++ args.extraRustcArgsForPlatform platform;
            # Manifest-derived flags come first so per-call `policy.clippy`
            # entries land later in argv and can override them. Cargo's
            # `[lints.clippy]` resolution is the load-bearing source for most
            # workspaces; `policy.clippy.deniedLints` stays as an escape hatch
            # for callers without a Cargo.toml policy.
            extraClippyLintArgs =
              clippyLintFlagsFromManifest (args.src + "/Cargo.toml") ++ rust.clippyLintArgs args.policy;
            clippyEnabled = perUnitClippyEnabled;
            extraPolicyChecks = extraPolicyChecksFromRust;
          }
          // seam
        );

      # The from-source units / libraries, before any prebuilt injection. Used
      # only to validate the injection keys; never built unless referenced.
      generatedView = importUnits {
        extraUnits = { };
        extraLibraries = { };
      };
      generatedUnitKeys = builtins.attrNames generatedView.units;
      generatedLibraryKeys = builtins.attrNames generatedView.libraries;

      # The workspace's ACTUAL toolchain id (cargo-unit.nix toolchainId at render
      # time), which is what every from-source unit hash was computed with. A
      # prebuilt unit must have been compiled with this exact toolchain, or its
      # hash (hence its key) would not match. `mkPrebuiltLibraryUnit` asserts
      # against its own `rustToolchain` arg; this is the workspace-side
      # cross-check against the toolchain the graph really used.
      workspaceToolchainId = builtins.baseNameOf (builtins.toString args.rustToolchain);

      # C1: a prebuilt injection must OVERRIDE a unit/library the graph already
      # references. A key that is absent silently builds from source, defeating
      # the feature with zero signal, so fail loud and name the offending key.
      # Returns a list of human-readable problem strings (empty when valid).
      injectionKeyProblems =
        label: injected: validKeys:
        let
          unknown = builtins.filter (key: !(builtins.elem key validKeys)) (builtins.attrNames injected);
        in
        lib.optional (unknown != [ ]) ''
          ${label} key(s) not present in the generated graph: ${lib.concatStringsSep ", " unknown}
          A prebuilt injection must override a unit the workspace already references; a
          missing key would silently build from source. Available ${label} keys:
            ${lib.concatStringsSep "\n  " validKeys}'';

      # C2: each injected unit must carry the workspace's actual toolchain id.
      # `mkPrebuiltLibraryUnit` records it in passthru; non-prebuilt injections
      # without that passthru are not checked (callers own those).
      injectionToolchainProblems =
        label: injected:
        let
          mismatched = lib.filterAttrs (
            _: unit: (unit.passthru.toolchainId or workspaceToolchainId) != workspaceToolchainId
          ) injected;
          render = key: unit: "${key} (compiled with ${unit.passthru.toolchainId or "?"})";
        in
        lib.optional (mismatched != { }) ''
          ${label} compiled with a toolchain other than this workspace's (${workspaceToolchainId}):
            ${lib.concatStringsSep "\n  " (lib.mapAttrsToList render mismatched)}
          A prebuilt rlib only links against, and only hashes to the same unit key as,
          the toolchain that produced it. Thread the workspace's rustToolchain into
          mkPrebuiltLibraryUnit.'';

      # All prebuilt-injection guard problems, gathered so a single assert can
      # report every offending key at once (and so the assert keeps its
      # `lib.assertMsg` shape, per the no-bare-assert lint).
      injectionProblems =
        injectionKeyProblems "extraUnits" args.extraUnits generatedUnitKeys
        ++ injectionKeyProblems "extraLibraries" args.extraLibraries generatedLibraryKeys
        ++ injectionToolchainProblems "extraUnits" args.extraUnits
        ++ injectionToolchainProblems "extraLibraries" args.extraLibraries;

      units =
        assert lib.assertMsg (injectionProblems == [ ]) (
          "cargoUnit.buildWorkspace: invalid prebuilt-unit injection:\n"
          + lib.concatStringsSep "\n" injectionProblems
        );
        importUnits { inherit (args) extraUnits extraLibraries; };
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
      # Per-crate policy gates. Each crate gets its own clippy and
      # unused-crate-dependency check (referencing only its own units) instead of
      # the workspace-wide aggregates, so editing one crate rebuilds only its own
      # checks. cargoAudit is lockfile-scoped (one Cargo.lock) and is exposed once
      # at the workspace level rather than aliased onto every crate.
      policyChecks =
        lib.optionalAttrs (
          (workspace.policy.clippy.enable or false) && (workspace.clippyByPackage or { }) ? ${packageName}
        ) { clippy = workspace.clippyByPackage.${packageName}; }
        // lib.optionalAttrs (
          (workspace.policy.denyUnusedCrateDependencies or false)
          && (workspace.unusedCrateDependenciesByPackage or { }) ? ${packageName}
        ) { unusedCrateDependencies = workspace.unusedCrateDependenciesByPackage.${packageName}; };
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

  /**
    Build a library unit derivation from already-compiled artifacts instead of
    from source.

    The result is byte-contract-identical to a library unit the renderer would
    emit (`packages/nix-cargo-unit/src/render.rs:1375-1402`): `$out` carries
    `$out/lib/lib<name>-<hash>.rlib`, the matching `.rmeta`, and
    `$out/nix-support/extern-path` holding the absolute path to the `.rlib`.
    A downstream unit therefore consumes it exactly like a from-source unit:
    `-L dependency=$out/lib` and `--extern <crate>=$(cat $out/nix-support/extern-path)`
    (`render.rs:1015-1047`).

    Pass the produced derivation through `buildWorkspace`'s `extraUnits` (keyed by
    `"<name>-<version>-<hash>"`). Because a unit's `<hash>` hashes package
    identity, target, edition, crate-types, features, profile, dependency
    identities, and the toolchain id, but never the source bytes
    (`model.rs:612-672`, `hash.rs:18-26`), a metadata-faithful stub crate yields
    the same `<hash>` as the real prebuilt, so injecting this unit links a
    downstream crate against a prebuilt rlib with no source present.

    Scope: this is for plain `rlib` libraries only. The artifact name and
    `extern-path` hardcode `.rlib`, so a `cdylib`, `staticlib`, or `proc-macro`
    crate (different artifact extension, and proc-macros load as host dylibs) is
    out of scope and would not link.

    Trust boundary: an injected prebuilt unit BYPASSES every per-unit policy gate
    (clippy, `--deny-panics`, unused-crate-dependencies) because those gates run
    on from-source compile units, not on a copied artifact. Inject only trusted
    artifacts (e.g. a first-party SDK rlib fetched from your own R2).

    `extraLibraries` is usually unnecessary: `buildWorkspace`'s `libraries` set
    derives from `units`, and a downstream crate links via `units.<key>`, so
    overriding `extraUnits.<key>` already routes the link through the prebuilt.
    Reach for `extraLibraries` only to make `workspace.libraries.<name>` itself
    point at the prebuilt (e.g. for `selectLibraryWithTests`).

    Arguments:
    - `name`: the library unit's Cargo target name (the leading component of the
      unit key), which for a default `lib` target is the underscored crate name
      (e.g. package `my-lib` has target `my_lib`). Any dashes are mapped to
      underscores for the on-disk artifact names, matching the renderer.
    - `version`: the crate version, used only to build the unit key the caller
      injects under.
    - `hash`: the source-independent unit hash. Must equal the `<hash>` the
      renderer computes for the metadata-faithful stub the downstream graph sees,
      or the downstream `--extern`/`-L` references will not resolve to this unit.
    - `rlib`: path to the compiled `.rlib` artifact.
    - `rmeta`: path to the compiled `.rmeta` artifact.
    - `toolchainId`: the toolchain id the prebuilt was compiled with. Asserted
      equal to `baseNameOf (toString rustToolchain)` so a toolchain mismatch
      fails at eval, never at link time. Also recorded in `passthru.toolchainId`
      so `buildWorkspace` can cross-check it against the workspace's actual
      toolchain at injection time.
    - `rustToolchain`: optional; defaults to `rust.defaultRustToolchain`. Used
      only for the toolchain-id assertion. A caller whose `buildWorkspace` uses a
      non-default toolchain MUST thread that same `rustToolchain` here, or the
      workspace-side cross-check in `buildWorkspace` will reject the injection.
    - `depUnits`: optional list of this prebuilt's own transitive dependency unit
      derivations, recorded to `$out/nix-support/dependency-units` for provenance.
      Defaults to `[ ]` (a leaf library, the validated path). NOTE: this is
      currently informational only and is NOT auto-injected into the consuming
      graph; a prebuilt with transitive deps still requires those dep units to be
      present in the consumer's graph (keyed by the same hash) and injected via
      `extraUnits`. Tracked in ENG-2166.
  */
  mkPrebuiltLibraryUnit =
    {
      name,
      version,
      hash,
      rlib,
      rmeta,
      toolchainId,
      rustToolchain ? rust.defaultRustToolchain,
      depUnits ? [ ],
    }:
    let
      expectedToolchainId = builtins.baseNameOf (builtins.toString rustToolchain);
      # The renderer underscores the Cargo target name for on-disk artifacts
      # (`render.rs:1376`). Mirror that exactly so the rlib filename and the
      # `extern-path` contents match what a from-source unit would produce.
      libName = builtins.replaceStrings [ "-" ] [ "_" ] name;
    in
    assert lib.assertMsg (toolchainId == expectedToolchainId) ''
      cargoUnit.mkPrebuiltLibraryUnit: toolchainId mismatch for `${name}`.
        prebuilt was compiled with: ${toolchainId}
        this workspace's toolchain: ${expectedToolchainId}
      A prebuilt rlib/rmeta only links against the toolchain that produced it.
    '';
    # M2: this builder is rlib-only (the filename and extern-path hardcode
    # `.rlib`). Reject an artifact that is clearly not an rlib/rmeta so a
    # cdylib/staticlib/proc-macro mistake fails loud at eval, not at link.
    assert lib.assertMsg (lib.hasSuffix ".rlib" (builtins.toString rlib)) ''
      cargoUnit.mkPrebuiltLibraryUnit: `rlib` for `${name}` must be a .rlib path; got ${builtins.toString rlib}.
      Only plain rlib libraries are supported (not cdylib/staticlib/proc-macro).
    '';
    assert lib.assertMsg (lib.hasSuffix ".rmeta" (builtins.toString rmeta)) ''
      cargoUnit.mkPrebuiltLibraryUnit: `rmeta` for `${name}` must be a .rmeta path; got ${builtins.toString rmeta}.
    '';
    pkgs.runCommand "cargo-unit-prebuilt-${name}-${version}-${hash}"
      {
        # Surfaced for callers/tests that want to confirm the injected key
        # without reconstructing the format string.
        passthru = {
          unitKey = "${name}-${version}-${hash}";
          libraryName = libName;
          inherit
            name
            version
            hash
            toolchainId
            ;
        };
      }
      ''
        mkdir -p "$out/lib" "$out/nix-support"
        cp ${lib.escapeShellArg (builtins.toString rlib)} "$out/lib/lib${libName}-${hash}.rlib"
        cp ${lib.escapeShellArg (builtins.toString rmeta)} "$out/lib/lib${libName}-${hash}.rmeta"
        # Same artifact priority as render.rs:1387-1398 (.rlib wins over .rmeta).
        printf '%s\n' "$out/lib/lib${libName}-${hash}.rlib" > "$out/nix-support/extern-path"
        ${lib.concatMapStringsSep "\n" (
          dep:
          ''printf '%s\n' ${lib.escapeShellArg (builtins.toString dep)} >> "$out/nix-support/dependency-units"''
        ) depUnits}
      '';
in
{
  inherit
    buildBinary
    buildBinaries
    buildWorkspace
    selectBinaryWithTests
    selectLibraryWithTests
    auditCargoLock
    defaultToolchainId
    generateUnitGraph
    generateUnitsNix
    mkPrebuiltLibraryUnit
    vendorDir
    vendorSources
    ;
}
