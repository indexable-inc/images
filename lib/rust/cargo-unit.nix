{
  lib,
  pkgs,
  nixCargoUnit,
  rust,
}:
let
  # The toolchain id baked into every unit hash for the default toolchain.
  # Exposed so callers of `mkPrebuiltLibraryUnit` can record and assert the id a
  # prebuilt rlib was compiled with without reconstructing it by hand. The id
  # rule itself lives at the toolchain owner (`rust.toolchainId`).
  defaultToolchainId = rust.toolchainId rust.defaultRustToolchain;

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

  # The shared "vendored cargo" context (src, cargoLock, toolchain, policy,
  # vendorDir, vendorSources, ...) is resolved by `rust.normalizeArgs`: build.nix
  # and cargoUnit are two consumers of one normalizer, so the lockfile, toolchain,
  # policy, and vendor resolution live there once. `buildWorkspace` normalizes its
  # raw args exactly once and hands the result, plus the unit-graph knobs
  # (`profile`, `target`, `contentAddressed`, `cargoTargets`), to the two IFD
  # stages; the stages no longer re-normalize. The remaining knobs
  # (`extraUnits`/`extraLibraries`, the `test*` forwarding) have a single reader
  # and are read from raw args at that use site.
  #
  # The one cross-stage value is the list of cargo invocations: the graph builder
  # and the target-set naming both need it, so `buildWorkspace` resolves it once
  # with this helper, which also enforces the non-empty invariant.
  cargoTargetsFor =
    rawArgs: cargoArgs:
    let
      cargoTargets = rawArgs.cargoTargets or [ cargoArgs ];
    in
    if cargoTargets == [ ] then
      throw "cargoUnit.buildWorkspace requires at least one cargoTargets entry"
    else
      cargoTargets;

  workspaceRootFor =
    args:
    args.workspaceRoot or (throw ''
      cargoUnit.buildWorkspace requires workspaceRoot = ./path/to/workspace.
      Use workspaceRoot for the real checkout root that package-shaped sources can be carved from.
      Fetched or patched sources pass workspaceRoot = src.
    '');

  renderCargoArgs =
    { profile, target }:
    cargoTarget:
    lib.escapeShellArgs (
      [
        "build"
        "--unit-graph"
        "-Z"
        "unstable-options"
      ]
      ++ profileArgs profile
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

  # First IFD stage for `buildWorkspace`: emit Cargo's `--unit-graph` JSON for
  # the vendored workspace, one cargo invocation per `cargoTargets` entry merged
  # into one graph. Takes the already-normalized `args` plus the unit-graph knobs
  # (`cargoTargets`, `profile`, `target`) resolved by its one caller, so the
  # shared context is normalized once rather than re-derived here.
  generateUnitGraph =
    {
      args,
      cargoTargets,
      profile,
      target,
    }:
    let
      renderTarget = renderCargoArgs { inherit profile target; };
      unitGraphFile = targetIndex: "$TMPDIR/unit-graph-${builtins.toString targetIndex}.json";
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
          inherit (args) vendorDir cargoExtraConfig cargoLock;
        }}

        cd ${args.src}

        pids=
        ${lib.concatStringsSep "\n" (
          lib.imap0 (targetIndex: targetArgs: ''
            (
              export CARGO_TARGET_DIR="$TMPDIR/cargo-target-${builtins.toString targetIndex}"
              cargo ${renderTarget targetArgs} > "${unitGraphFile targetIndex}"
            ) &
            pids="$pids $!"
          '') cargoTargets
        )}

        for pid in $pids; do
          wait "$pid"
        done

        nix-cargo-unit merge ${
          lib.concatStringsSep " " (lib.imap0 (targetIndex: _: unitGraphFile targetIndex) cargoTargets)
        } > "$out"
      '';

  # Second IFD stage for `buildWorkspace`: render `units.nix` from the unit graph
  # `generateUnitGraph` produced. Separate derivation so the graph and the render
  # are independently inspectable (both are surfaced on the workspace output).
  # Takes the normalized `args`, the graph as an explicit input, and the
  # `contentAddressed` knob from its one caller.
  generateUnitsNix =
    {
      args,
      unitGraphJson,
      contentAddressed,
    }:
    let
      toolchainId = rust.toolchainId args.rustToolchain;
      cargoLockForRender = rust.cargoLockFile args.cargoLock;
      renderFlags = [
        "render"
        "--workspace-root"
        (builtins.toString args.src)
        "--vendor-root"
        (builtins.toString args.vendorDir)
        "--toolchain-id"
        toolchainId
      ]
      ++ lib.optional contentAddressed "--content-addressed"
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
    changes (profile, policy, rustToolchain, env, extraRustcArgs). Top-level
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

    Returns the generated attrset with `sourceAudit`, `units`, `roots`, `checkedRoots`,
    `packages`, `binaries`, `libraries`, `benchmarks`, `coverageReport`, `default`,
    `policyChecks`, plus the intermediate `unitGraphJson`, `unitsNix`, and `vendorDir`
    derivations for inspection.
  */
  buildWorkspace =
    rawArgs:
    let
      args = rust.normalizeArgs rawArgs;
      inherit (args) vendorDir vendorSources;
      workspaceRoot = workspaceRootFor rawArgs;
      cargoTargets = cargoTargetsFor rawArgs args.cargoArgs;
      cargoTargetNames = rawArgs.cargoTargetNames or null;
      extraUnits = rawArgs.extraUnits or { };
      extraLibraries = rawArgs.extraLibraries or { };
      unitGraphJson = generateUnitGraph {
        inherit args cargoTargets;
        profile = rawArgs.profile or "release";
        target = rawArgs.target or null;
      };
      unitsNix = generateUnitsNix {
        inherit args unitGraphJson;
        contentAddressed = rawArgs.contentAddressed or true;
      };
      perUnitClippyEnabled = args.policy.clippy.enable;
      # Per-unit clippy runs `clippy-driver` directly on each non-external
      # unit. Suppress the legacy workspace-level `cargoClippy` derivation in
      # that mode so the same lints don't run twice and so a single source
      # edit doesn't invalidate every other crate's clippy.
      extraPolicyChecksFromRust = rust.policyChecksFor (
        rawArgs
        // {
          inherit vendorDir;
          # A workspace has no single crate name; name the workspace-level checks
          # explicitly rather than relying on a fallback (`crateName` requires it).
          pname = rawArgs.pname or "cargo-unit-workspace";
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
            inherit (args) src rustToolchain;
            extraRustcArgs = rawArgs.extraRustcArgs or [ ];
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
            testRunPrelude = rawArgs.testRunPrelude or "";
            testArgsByPackage = rawArgs.testArgsByPackage or { };
            packageTestInputs = rawArgs.packageTestInputs or { };
            packageTestEnv = rawArgs.packageTestEnv or { };
            extraRustcArgsForPlatform =
              platform:
              rust.rustcArgsForPolicyForPlatform args.policy platform
              ++ (rawArgs.extraRustcArgsForPlatform or (_platform: [ ])) platform;
            # Manifest-derived flags come first so per-call `policy.clippy`
            # entries land later in argv and can override them. Cargo's
            # `[lints.clippy]` resolution is the load-bearing source for most
            # workspaces; `policy.clippy.deniedLints` stays as an escape hatch
            # for callers without a Cargo.toml policy.
            extraClippyLintArgs =
              rust.clippyLintFlagsFromManifest (args.src + "/Cargo.toml") ++ rust.clippyLintArgs args.policy;
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
      workspaceToolchainId = rust.toolchainId args.rustToolchain;

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
        injectionKeyProblems "extraUnits" extraUnits generatedUnitKeys
        ++ injectionKeyProblems "extraLibraries" extraLibraries generatedLibraryKeys
        ++ injectionToolchainProblems "extraUnits" extraUnits
        ++ injectionToolchainProblems "extraLibraries" extraLibraries;

      units =
        assert lib.assertMsg (injectionProblems == [ ]) (
          "cargoUnit.buildWorkspace: invalid prebuilt-unit injection:\n"
          + lib.concatStringsSep "\n" injectionProblems
        );
        importUnits { inherit extraUnits extraLibraries; };
      targetSetNames =
        if cargoTargetNames == null then
          lib.genList builtins.toString (builtins.length cargoTargets)
        else if builtins.length cargoTargetNames == builtins.length cargoTargets then
          cargoTargetNames
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
    workspace.binaries.${binary}
      or (throw "buildBinary: no binary `${binary}` in workspace; available: ${
        lib.concatStringsSep ", " (builtins.attrNames (workspace.binaries or { }))
      }");

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
      rootDrv =
        workspace.binaries.${binary}
          or (throw "selectBinaryWithTests: no binary `${binary}` in workspace; available: ${
            lib.concatStringsSep ", " (builtins.attrNames (workspace.binaries or { }))
          }");
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
      # `buildWorkspace` always sets `policy` via `resolvePolicy`, so the policy
      # flags are present. The per-package maps come from the nix-cargo-unit
      # renderer and are genuinely absent when it emitted none, so those stay
      # guarded.
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
    {
      binaries,
      cargoArgs ? [ ],
      ...
    }@args:
    let
      workspace = buildWorkspace (builtins.removeAttrs args [ "binaries" ]);
    in
    lib.genAttrs binaries (
      binary:
      workspace.binaries.${binary}
        or (throw "buildBinaries: no binary `${binary}` in workspace; available: ${
          lib.concatStringsSep ", " (builtins.attrNames (workspace.binaries or { }))
        }")
    );

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
      expectedToolchainId = rust.toolchainId rustToolchain;
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
    defaultToolchainId
    mkPrebuiltLibraryUnit
    ;
}
