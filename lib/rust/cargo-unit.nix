{
  lib,
  pkgs,
  nixCargoUnit,
  rust,
}:
let
  inherit (builtins)
    attrNames
    elem
    filter
    genericClosure
    hasAttr
    head
    length
    removeAttrs
    replaceStrings
    toString
    ;

  inherit (lib) escapeShellArg;

  # The toolchain id baked into every unit hash for the default toolchain.
  # Exposed so callers of `mkPrebuiltLibraryUnit` can record and assert the id a
  # prebuilt rlib was compiled with without reconstructing it by hand. The id
  # rule itself lives at the toolchain owner (`rust.toolchainId`).
  defaultToolchainId = rust.toolchainId rust.defaultRustToolchain;

  # Apply the rustflags a normal `cargo build` reads from `.cargo/config.toml`,
  # which cargoUnit otherwise ignores (it assembles rustc args itself instead of
  # going through cargo). Parsing the config here is the only route: cargo's
  # `cargo build --unit-graph` does NOT carry rustflags (each unit records only
  # dependencies/features/mode/pkg_id/platform/profile/target), because cargo
  # resolves config rustflags at compile time and applies them when it invokes
  # rustc, which cargoUnit bypasses by invoking rustc per unit from the graph. So
  # there is nothing in the graph to pick up automatically; we read the config.
  # Returns the rustc args for a target triple following cargo precedence:
  # `target.<triple>.rustflags` wins outright over `build.rustflags` (cargo does
  # not merge the two). Flags may be a TOML array or a single whitespace-
  # separated string. `cfg(...)` target sections and the `[env]` table are NOT
  # honored (cargo evaluates those against the full target cfg set, which this
  # static parse does not reproduce). A `configPath` that does not exist yields
  # no flags, so callers may pass the path unconditionally.
  rustflagsFromCargoConfig =
    configPath: platform:
    let
      config = lib.importTOML configPath;
      normalize =
        flags:
        if builtins.isList flags then flags else filter (flag: flag != "") (lib.splitString " " flags);
      chosen = config.target.${platform}.rustflags or config.build.rustflags or null;
    in
    # Lazy: the `&&` short-circuits, so `config` (hence `importTOML`) is only
    # forced when the file exists and carries rustflags.
    if builtins.pathExists configPath && chosen != null then normalize chosen else [ ];

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
    Roots are consumed lazily: `binaries.<name>`, `libraries.<name>`, and
    `targetSets.<set>.*` each reference one rustc unit derivation, so selecting
    a subset of roots (say the native cdylibs out of a graph that also plans a
    wasm target) never builds the other entries' units. A second buildWorkspace
    call that only narrows `cargoTargets` yields byte-identical root
    derivations (pinned by a tests/default.nix assertion) and adds a unit-graph
    plus render IFD; create a separate workspace only when unit identity
    changes (profile, policy, rustToolchain, env, extraRustcArgs). `env` folds
    into every unit, so a value that changes often (a baked git commit) busts the
    whole dependency closure; scope it with `packageBuildEnv.<package> = { ... }`
    instead, which only reaches that package's own compile and build-script-run
    units. Top-level
    `binaries`/`libraries` dedupe by Cargo target name and the first
    `cargoTargets` entry wins, so when one crate roots under several entries,
    select through `targetSets.<set>` instead. Per-case discovery is the
    exception to per-root laziness: `tests.<target>.cases` uses a shared
    manifest IFD that builds every test binary in the graph, and
    `doctests.<target>.cases` uses a shared doctest manifest covering every
    doctest target.
    Include `--benches` or `--bench <name>` to expose `[[bench]]` roots under
    `benchmarks` and `benchmarkPlan`. Tango benches can compare previous and
    next artifacts with `next.compareTangoBenchmarks { baseline = previous; }`,
    where `previous` is another generated workspace or a `benchmarkPlan` path.
    Test graphs also expose `coverageReport` and `makeCoverageReport`; build the
    workspace with `extraRustcArgs = [ "-Cinstrument-coverage" ]` and consume the
    generated `$out/lcov.info`. The selected Rust toolchain must provide matching
    `llvm-cov` and `llvm-profdata`, or callers must pass explicit tool paths to
    `makeCoverageReport`.

    `cargoConfigRustflags = true` applies the rustflags a normal `cargo build`
    would read from `<workspaceRoot>/.cargo/config.toml` (cargoUnit otherwise
    ignores cargo's config). Flags are resolved per target triple with cargo
    precedence (`target.<triple>.rustflags` over `build.rustflags`); `cfg(...)`
    target sections and the `[env]` table are not honored. Default off.

    Returns the generated attrset with `sourceAudit`, `units`, `roots`, `checkedRoots`,
    `packages`, `binaries`, `libraries`, `benchmarks`, `coverageReport`, `default`,
    `policyChecks`, plus the intermediate `unitGraphJson`, `unitsNix`, and `vendorDir`
    derivations for inspection.

    `rust.resolveArgs` resolves the shared bundle (context, policy, linker,
    effects, checks) once; the two IFD stages and the unit import below read the
    once-resolved values (configScript, toolchainId, cargoLockPath, render flags,
    mold/clippy args, workspace checks) straight off it. The remaining knobs
    (`profile`, `target`, `contentAddressed`, `cargoTargets`,
    `extraUnits`/`extraLibraries`, the `test*` forwarding) each have a single
    reader and are read from raw args at that use site.
  */
  buildWorkspace =
    rawArgs:
    let
      resolved = rust.resolveArgs rawArgs;
      inherit (resolved)
        context
        effects
        policy
        checks
        ;
      # A flat view of the resolved context for the field readers below; the
      # once-resolved values (configScript, toolchainId, cargoLockPath, render
      # flags, mold args, clippy args, checks) are read straight off the bundle.
      args = context // {
        inherit policy;
        inherit (resolved) cargoArgs;
      };
      inherit (args) vendorDir vendorSources;

      workspaceRoot =
        rawArgs.workspaceRoot or (throw ''
          cargoUnit.buildWorkspace requires workspaceRoot = ./path/to/workspace.
          Use workspaceRoot for the real checkout root that package-shaped sources can be carved from.
          Fetched or patched sources pass workspaceRoot = src.
        '');

      # The list of cargo invocations to plan: the graph builder and the
      # target-set naming both consume it, and it must be non-empty.
      cargoTargets =
        let
          targets = rawArgs.cargoTargets or [ args.cargoArgs ];
        in
        if targets == [ ] then
          throw "cargoUnit.buildWorkspace requires at least one cargoTargets entry"
        else
          targets;

      explicitExtraUnits = rawArgs.extraUnits or { };
      extraLibraries = rawArgs.extraLibraries or { };

      # Every injected unit plus everything reachable from one through
      # `passthru.depUnits` (recorded by `mkPrebuiltLibraryUnit`), deduplicated
      # by derivation. A recorded dep whose unit key the caller explicitly
      # pinned in `extraUnits` is pruned BEFORE descending: the pinned
      # derivation (already a closure root) is the selected unit for that key,
      # and the discarded dep's own subtree must not auto-inject units or
      # raise conflicts on behalf of an artifact the graph never links.
      # Walking by drvPath rather than unitKey keeps two distinct derivations
      # that claim the same unpinned key visible to the conflict guard below
      # instead of silently dropping one of them.
      injectedUnitClosure = map (item: item.unit) (genericClosure {
        startSet = lib.mapAttrsToList (_: unit: {
          key = unit.drvPath;
          inherit unit;
        }) explicitExtraUnits;
        operator =
          item:
          map
            (dep: {
              key = dep.drvPath;
              unit = dep;
            })
            (
              filter (dep: !(hasAttr (dep.passthru.unitKey or "") explicitExtraUnits)) (
                item.unit.passthru.depUnits or [ ]
              )
            );
      });

      # The closure grouped by recorded unit key. Injected units without a
      # `passthru.unitKey` (arbitrary caller-owned derivations) record no key
      # and never participate in auto-injection.
      injectedUnitsByKey = lib.groupBy (unit: unit.passthru.unitKey) (
        filter (unit: unit ? passthru.unitKey) injectedUnitClosure
      );

      # Transitive deps of the injected prebuilts, auto-injected under their own
      # recorded unit keys so a caller injects only the root unit (ENG-2166).
      # An explicit `extraUnits` entry wins the merge below, so a caller can
      # deliberately pin one dep key to a different artifact.
      autoInjectedDepUnits = lib.mapAttrs (_: head) (
        lib.filterAttrs (key: _: !(hasAttr key explicitExtraUnits)) injectedUnitsByKey
      );

      extraUnits = autoInjectedDepUnits // explicitExtraUnits;

      # First IFD stage: emit Cargo's `--unit-graph` JSON for the vendored
      # workspace, one cargo invocation per `cargoTargets` entry merged into one
      # graph. Separate derivation from the render so both are independently
      # inspectable on the workspace output.
      unitGraphJson =
        let
          profile = rawArgs.profile or "release";
          target = rawArgs.target or null;

          profileArgs =
            {
              release = [ "--release" ];
              dev = [ ];
            }
            ."${profile}" or [
              "--profile"
              profile
            ];
          renderTarget =
            cargoTarget:
            lib.escapeShellArgs (
              [
                "build"
                "--unit-graph"
                "-Z"
                "unstable-options"
              ]
              ++ profileArgs
              ++ lib.optionals (target != null) [
                "--target"
                target
              ]
              ++ cargoTarget
              ++ [
                "--frozen"
                "--offline"
              ]
            );
          unitGraphFile = targetIndex: "$TMPDIR/unit-graph-${toString targetIndex}.json";

          inherit (context) configScript;
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
              # This keeps the input graph generation local to the IFD planner
              # derivation instead of requiring a flake-wide Rust overlay.
              RUSTC_BOOTSTRAP = "1";
            }
            // args.env
          )
          ''
            ${configScript}

            cd ${args.src}

            pids=
            ${lib.concatStringsSep "\n" (
              lib.imap0 (targetIndex: targetArgs: ''
                (
                  export CARGO_TARGET_DIR="$TMPDIR/cargo-target-${toString targetIndex}"
                  cargo ${renderTarget targetArgs} > "${unitGraphFile targetIndex}"
                ) &
                pids="$pids $!"
              '') cargoTargets
            )}

            for pid in $pids; do
              wait "$pid"
            done

            nix-cargo-unit merge ${lib.concatStringsSep " " (lib.genList unitGraphFile (length cargoTargets))} > "$out"
          '';

      # The workspace's toolchain id, handed to the renderer and baked into
      # every from-source unit hash. A prebuilt unit must have been compiled
      # with this exact toolchain, or its hash (hence its key) would not
      # match. `mkPrebuiltLibraryUnit` asserts against its own `rustToolchain`
      # arg; the injection guards below cross-check against this id, the one
      # the graph really used. Sourced from the resolved context so the id is
      # derived once at the resolution boundary, not re-spelled here.
      workspaceToolchainId = context.toolchainId;

      # Second IFD stage: render `units.nix` from the unit graph above.
      unitsNix =
        let
          contentAddressed = rawArgs.contentAddressed or true;

          extraFlags = lib.optional contentAddressed "--content-addressed" ++ effects.renderFlags;
        in
        pkgs.runCommand "cargo-units.nix"
          {
            nativeBuildInputs = [ nixCargoUnit ];
            cargoLockForRender = context.cargoLockPath;
          }
          ''
            nix-cargo-unit render \
              --workspace-root ${escapeShellArg args.src} \
              --vendor-root ${escapeShellArg args.vendorDir} \
              --toolchain-id ${escapeShellArg workspaceToolchainId} \
              ${lib.escapeShellArgs extraFlags} \
              --cargo-lock "$cargoLockForRender" \
              < ${unitGraphJson} \
              > "$out"
          '';

      perUnitClippyEnabled = args.policy.clippy.enable;
      # Workspace-level policy checks: audit + machete only. Clippy is NOT here;
      # it runs per unit in the renderer (`clippyByPackage`), so a whole-workspace
      # `cargo clippy` would duplicate it and make one source edit invalidate every
      # crate's clippy. `workspaceChecks` omits it by construction (no suppression).
      # A workspace has no single crate name; name the checks explicitly.
      extraPolicyChecksFromRust = checks.workspace (rawArgs.pname or "cargo-unit-workspace");
      # Import the rendered units.nix with a given prebuilt-injection seam. The
      # generated (pre-seam) set is obtained by importing with empty seam args,
      # so the injection guards below can compare against the real generated keys
      # without a second IFD (the import is memoized; only the function call
      # differs). See mkPrebuiltLibraryUnit.
      importUnits =
        seam:
        let
          # The renderer passes `null` for host units (build scripts, proc-macros)
          # that have no `--target`; resolve that to the host triple before handing
          # it to the policy hook, which deliberately rejects a non-triple platform.
          extraRustcArgsForPlatform =
            platform:
            let
              resolvedPlatform = if platform == null then pkgs.stdenv.hostPlatform.config else platform;
            in
            effects.rustcArgsForPlatform resolvedPlatform
            ++ (rawArgs.extraRustcArgsForPlatform or (_platform: [ ])) platform
            # Opt-in: apply `.cargo/config.toml` rustflags (per target triple,
            # cargo precedence) so consumers do not hand-copy them into
            # `extraRustcArgs`. Appended last so explicit caller args still win.
            ++ lib.optionals (rawArgs.cargoConfigRustflags or false) (
              rustflagsFromCargoConfig (workspaceRoot + "/.cargo/config.toml") resolvedPlatform
            );
        in
        import unitsNix (
          {
            inherit pkgs vendorDir vendorSources;
            inherit (args) src rustToolchain;
            extraRustcArgs = rawArgs.extraRustcArgs or [ ];
            inherit workspaceRoot;
            # Scanner for the opt-in panic-freedom policy. The rendered check
            # asserts this is non-null when `policy.denyPanics` is set.
            cargoUnit = nixCargoUnit;
            extraNativeBuildInputs = args.nativeBuildInputs ++ effects.linkerNativeInputs;
            # `clippy-driver` ships in the clippy package; `rustToolchain` only
            # guarantees rustc + cargo. Adding the resolved clippy package keeps
            # version drift impossible because the toolchain pins the rustc that
            # `clippy-driver` links against.
            extraClippyNativeBuildInputs = lib.optional perUnitClippyEnabled args.policy.clippy.package;
            extraEnv = args.env;
            testRunPrelude = rawArgs.testRunPrelude or "";
            testArgsByPackage = rawArgs.testArgsByPackage or { };
            packageTestInputs = rawArgs.packageTestInputs or { };
            packageTestEnv = rawArgs.packageTestEnv or { };
            packageBuildEnv = rawArgs.packageBuildEnv or { };
            inherit extraRustcArgsForPlatform;
            # Manifest-derived flags come first so per-call `policy.clippy`
            # entries land later in argv and can override them. Cargo's
            # `[lints.clippy]` resolution is the load-bearing source for most
            # workspaces; `policy.clippy.deniedLints` stays as an escape hatch
            # for callers without a Cargo.toml policy.
            extraClippyLintArgs =
              rust.clippyLintFlagsFromManifest (args.src + "/Cargo.toml") ++ effects.clippyLintArgs;
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
      generatedUnitKeys = attrNames generatedView.units;
      generatedLibraryKeys = attrNames generatedView.libraries;

      # C1: a prebuilt injection must OVERRIDE a unit/library the graph already
      # references. A key that is absent silently builds from source, defeating
      # the feature with zero signal, so fail loud and name the offending key.
      # Returns a list of human-readable problem strings (empty when valid).
      injectionKeyProblems =
        label: injected: validKeys:
        let
          unknown = filter (key: !(elem key validKeys)) (attrNames injected);
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

      # C3: when an explicitly injected unit records its own unit key, the
      # caller's chosen attr key must agree with it. The artifact names inside
      # the unit embed that key's hash, and auto-injection keys the unit's deps
      # by `passthru.unitKey`, so a disagreement would inject one derivation
      # under two keys.
      injectionUnitKeyMismatchProblems =
        let
          mismatched = lib.filterAttrs (key: unit: (unit.passthru.unitKey or key) != key) explicitExtraUnits;
          render = key: unit: "${key} (the unit's own passthru.unitKey is ${unit.passthru.unitKey})";
        in
        lib.optional (mismatched != { }) ''
          extraUnits key(s) that disagree with the injected unit's recorded unitKey:
            ${lib.concatStringsSep "\n  " (lib.mapAttrsToList render mismatched)}
          A prebuilt unit must be injected under its `passthru.unitKey`; the rlib and
          extern-path inside it are named for that key's hash.'';

      # C4: two recorded prebuilts claiming one unit key with different
      # derivations is ambiguous, and whichever the graph linked would be a
      # silent choice. An explicit `extraUnits` entry for the key resolves the
      # ambiguity (it wins the merge), so only unpinned keys are problems.
      depUnitConflictProblems =
        let
          conflicts = lib.filterAttrs (
            key: unitDrvs: length unitDrvs > 1 && !(hasAttr key explicitExtraUnits)
          ) injectedUnitsByKey;
          render =
            key: unitDrvs: "${key}:\n    ${lib.concatMapStringsSep "\n    " (unit: unit.drvPath) unitDrvs}";
        in
        lib.optional (conflicts != { }) ''
          conflicting prebuilt derivations recorded for the same dependency unit key:
            ${lib.concatStringsSep "\n  " (lib.mapAttrsToList render conflicts)}
          Two injected prebuilt units recorded different derivations for one transitive
          dep (`passthru.depUnits`). Pin the key in extraUnits explicitly to choose one.'';

      # All prebuilt-injection guard problems, gathered so a single assert can
      # report every offending key at once (and so the assert keeps its
      # `lib.assertMsg` shape, per the no-bare-assert lint).
      injectionProblems =
        injectionKeyProblems "extraUnits" explicitExtraUnits generatedUnitKeys
        ++ injectionKeyProblems "extraUnits (auto-injected depUnits)" autoInjectedDepUnits generatedUnitKeys
        ++ injectionKeyProblems "extraLibraries" extraLibraries generatedLibraryKeys
        ++ injectionToolchainProblems "extraUnits" extraUnits
        ++ injectionToolchainProblems "extraLibraries" extraLibraries
        ++ injectionUnitKeyMismatchProblems
        ++ depUnitConflictProblems;

      units =
        assert lib.assertMsg (injectionProblems == [ ]) (
          "cargoUnit.buildWorkspace: invalid prebuilt-unit injection:\n"
          + lib.concatStringsSep "\n" injectionProblems
        );
        importUnits { inherit extraUnits extraLibraries; };
      targetSetNames =
        let
          targetCount = length cargoTargets;
        in
        if rawArgs ? cargoTargetNames then
          let
            names = rawArgs.cargoTargetNames;
          in
          assert lib.assertMsg (
            length names == targetCount
          ) "cargoUnit.buildWorkspace requires cargoTargetNames to match cargoTargets length";
          names
        else
          lib.genList toString targetCount;
      namedTargetSets = lib.listToAttrs (
        lib.zipListsWith lib.nameValuePair targetSetNames units.targetSets
      );
    in
    units
    // {
      inherit unitGraphJson unitsNix vendorDir;
      targetSets = namedTargetSets;
      inherit (args) policy;
    };

  # One lookup for every selector that picks a root out of a workspace: fail
  # with the calling selector's name and the full set of available keys, so a
  # typo'd target name reads as a menu instead of a bare missing-attribute
  # error.
  rootOrThrow =
    caller: kind: roots: name:
    roots.${name}
      or (throw "${caller}: no ${kind} `${name}` in workspace; available: ${lib.concatStringsSep ", " (attrNames roots)}");

  /**
    Select one binary target from a generated workspace graph.
  */
  buildBinary =
    { binary, ... }@args:
    let
      workspace = buildWorkspace (removeAttrs args [ "binary" ]);
    in
    rootOrThrow "buildBinary" "binary" (workspace.binaries or { }) binary;

  /**
    Pick a binary out of a pre-built `buildWorkspace` plus its test
    derivations, ready for `passthru.tests` consumption.

    Test and doctest targets are every generated target owned by `packageName`.
    Each discovered test case becomes its own derivation by default;
    `<target>-all` remains available for callers that need the full harness as a
    single compatibility check.

    Use this when the caller has one shared workspace (`ix.rustWorkspace.units`)
    so all repo-owned crates ride the same unit graph. Use `buildBinary` when
    a crate needs its own workspace (different policy, fetched source, etc).
  */
  selectBinaryWithTests =
    workspace:
    {
      binary,
      packageName ? binary,
      includeTestCases ? true,
      meta ? { },
      passthru ? { },
    }:
    selectRootWithTests workspace {
      rootDrv = rootOrThrow "selectBinaryWithTests" "binary" (workspace.binaries or { }) binary;
      inherit
        packageName
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
      includeTestCases ? true,
      meta ? { },
      passthru ? { },
    }:
    selectRootWithTests workspace {
      rootDrv = rootOrThrow "selectLibraryWithTests" "library" (workspace.libraries or { }) library;
      inherit
        packageName
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
      includeTestCases ? true,
      meta ? { },
      passthru ? { },
    }:
    let
      uncheckedRoot = rootDrv.passthru.unchecked or rootDrv;
      namesForPackage =
        attrName: fallback:
        if hasAttr attrName workspace && hasAttr packageName workspace.${attrName} then
          workspace.${attrName}.${packageName}
        else
          fallback;
      selectedTestTargets = namesForPackage "testTargetNamesByPackage" defaultTestTargets;
      selectedDoctestTargets = namesForPackage "doctestTargetNamesByPackage" [ ];
      flattenAllTargets =
        prefix: targetNames: targets:
        lib.mapAttrs' (targetName: target: lib.nameValuePair "${prefix}${targetName}-all" target.all) (
          lib.getAttrs (filter (name: targets ? ${name}) targetNames) targets
        );
      flattenCaseTargets =
        prefix: targetNames: targets:
        lib.concatMapAttrs (
          targetName: target:
          lib.mapAttrs' (
            case: drv:
            lib.nameValuePair "${prefix}${targetName}-${lib.replaceStrings [ "::" ] [ "-" ] case}" drv
          ) (target.cases or { })
        ) (lib.getAttrs (filter (name: targets ? ${name}) targetNames) targets);
      # Per-crate policy gates. Each crate gets its own clippy and
      # unused-crate-dependency check (referencing only its own units) instead of
      # the workspace-wide aggregates, so editing one crate rebuilds only its own
      # checks. cargoAudit is lockfile-scoped (one Cargo.lock) and is exposed once
      # at the workspace level rather than aliased onto every crate.
      # `buildWorkspace` always sets `policy`, so the policy flags are present.
      # The per-package maps come from the nix-cargo-unit renderer and are
      # genuinely absent when it emitted none, so those stay guarded.
      policyChecks =
        lib.optionalAttrs (
          workspace.policy.clippy.enable && (workspace.clippyByPackage or { }) ? ${packageName}
        ) { clippy = workspace.clippyByPackage.${packageName}; }
        // lib.optionalAttrs (
          workspace.policy.denyUnusedCrateDependencies
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
    { binaries, ... }@args:
    let
      workspace = buildWorkspace (removeAttrs args [ "binaries" ]);
    in
    lib.genAttrs binaries (rootOrThrow "buildBinaries" "binary" (workspace.binaries or { }));

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
    - `depUnits`: this prebuilt's own dependency unit derivations, each built
      with `mkPrebuiltLibraryUnit` (each entry must carry `passthru.unitKey`).
      Direct deps that each record their own `depUnits`, or a flattened
      transitive list, inject identically. Defaults to `[ ]` (a leaf library).
      `buildWorkspace` walks `passthru.depUnits` transitively and auto-injects
      every recorded unit into the consuming graph under its own
      `passthru.unitKey`, so the caller injects only the root unit. Each
      auto-injected key must name a unit the consumer's graph already
      references (the C1 guard), which holds exactly when the consumer's
      manifest pins the dependency closure the prebuilt was compiled against:
      the unit hash folds in dependency hashes recursively, so a root key match
      implies every dep key matches. An explicit `extraUnits` entry for a dep
      key overrides the recorded derivation. The deps are also recorded to
      `$out/nix-support/dependency-units` for provenance.
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
      expectedToolchainId = rust.toolchainId rustToolchain;
      # The renderer underscores the Cargo target name for on-disk artifacts
      # (`render.rs:1376`). Mirror that exactly so the rlib filename and the
      # `extern-path` contents match what a from-source unit would produce.
      libName = replaceStrings [ "-" ] [ "_" ] name;
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
    assert lib.assertMsg (lib.hasSuffix ".rlib" (toString rlib)) ''
      cargoUnit.mkPrebuiltLibraryUnit: `rlib` for `${name}` must be a .rlib path; got ${toString rlib}.
      Only plain rlib libraries are supported (not cdylib/staticlib/proc-macro).
    '';
    assert lib.assertMsg (lib.hasSuffix ".rmeta" (toString rmeta)) ''
      cargoUnit.mkPrebuiltLibraryUnit: `rmeta` for `${name}` must be a .rmeta path; got ${toString rmeta}.
    '';
    # Auto-injection keys each dep by its `passthru.unitKey`, so an entry
    # without one could never be wired into a consuming graph. Reject it at
    # construction, naming the offender, instead of at injection time.
    assert lib.assertMsg (filter (dep: !(dep ? passthru.unitKey)) depUnits == [ ]) ''
      cargoUnit.mkPrebuiltLibraryUnit: depUnits for `${name}` must be prebuilt unit
      derivations carrying `passthru.unitKey` (build them with mkPrebuiltLibraryUnit); got:
        ${lib.concatMapStringsSep "\n  " (dep: dep.name or "<non-derivation>") (
          filter (dep: !(dep ? passthru.unitKey)) depUnits
        )}
    '';
    pkgs.runCommand "cargo-unit-prebuilt-${name}-${version}-${hash}"
      {
        # Surfaced for callers/tests that want to confirm the injected key
        # without reconstructing the format string. `depUnits` is what
        # `buildWorkspace` walks to auto-inject this unit's transitive deps.
        passthru = {
          unitKey = "${name}-${version}-${hash}";
          libraryName = libName;
          inherit
            name
            version
            hash
            toolchainId
            depUnits
            ;
        };
      }
      ''
        mkdir -p "$out/lib" "$out/nix-support"
        cp ${lib.escapeShellArg (toString rlib)} "$out/lib/lib${libName}-${hash}.rlib"
        cp ${lib.escapeShellArg (toString rmeta)} "$out/lib/lib${libName}-${hash}.rmeta"
        # Same artifact priority as render.rs:1387-1398 (.rlib wins over .rmeta).
        printf '%s\n' "$out/lib/lib${libName}-${hash}.rlib" > "$out/nix-support/extern-path"
        ${lib.concatMapStringsSep "\n" (
          dep: ''printf '%s\n' ${lib.escapeShellArg (toString dep)} >> "$out/nix-support/dependency-units"''
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
    defaultToolchainId
    mkPrebuiltLibraryUnit
    ;
  # Named partial policies (e.g. `policyPresets.pureBuild`) for callers that build
  # pure artifacts and want to reference one name instead of re-spelling the gates.
  inherit (rust) policyPresets;
}
