# Per-system flake outputs (packages / checks / formatter).
#
# Kept out of flake.nix so the flake top-level can read as a manifest of
# inputs and output categories. Composition logic for workflow tools and
# lint plumbing lives here. Workflow tools (lint, update-mods, ...) are
# exposed under `packages.<system>.<name>` with `meta.mainProgram` set, so
# `nix run .#<name>` and `nix build .#<name>` both work without an `apps`
# entry (see AGENTS.md "Flake.nix style").
{
  system,
  ix,
  nixpkgs,
  paths,
  rust-overlay,
  home-manager,
}: let
  inherit (nixpkgs) lib;
  pkgs = import nixpkgs {
    inherit system;
    config = {};
    overlays = [
      rust-overlay.overlays.default
      ix.overlay
    ];
  };
  fs = lib.fileset;
  packageRegistry = import (paths.packagesRoot + "/registry.nix") {
    inherit lib;
    root = paths.packagesRoot;
    inherit (ix.lists) findDuplicates;
  };

  updateMods = ix.writePythonApplication pkgs {
    name = "update-mods";
    src = paths.tools.updateMods;
    pyChecker = "zuban";
    # pydantic validates Modrinth API responses at the boundary so upstream
    # drift fails with a path-precise error rather than a bare KeyError.
    python = pkgs.python314.withPackages (ps: [ps.pydantic]);
    meta.description = "Regenerate Minecraft mod catalogs";
  };

  updateLoaders = ix.writePythonApplication pkgs {
    name = "update-loaders";
    src = paths.tools.updateLoaders;
    pyChecker = "zuban";
    # pydantic validates the PaperMC fill v3 response at the boundary so upstream
    # drift fails with a path-precise error rather than a bare KeyError.
    python = pkgs.python314.withPackages (ps: [ps.pydantic]);
    meta.description = "Refresh Minecraft loader (Paper / Velocity / Fabric) catalogs from upstream";
  };

  ixShellSyncIgnored = ix.writePythonApplication pkgs {
    name = "ix-shell-sync-ignored";
    src = paths.tools.ixShellSyncIgnored;
    pyChecker = "zuban";
    runtimeInputs = [
      pkgs.git
      pkgs.gnutar
    ];
    meta.description = "Copy git-ignored files into an ix shell workspace";
  };

  # `nix run .#cve-scan`: scan the whole Nix closure of the repo's key outputs
  # One symlink-free directory holding every skill under `skills/`, ready to
  # copy into `.claude/skills`.
  skillsDir = ix.skills.mkSkillsDir {inherit pkgs;};

  # The `index` Claude Code plugin: every index skill bundled for `--plugin-dir`,
  # invoked as `/index:<skill>`. This is the pure-index default (no hooks, no
  # personal skills); a consumer wanting extras calls `ix.claudePlugin.mkPlugin`
  # with `extraSkills`/`hooks` directly.
  claudePluginDir = ix.claudePlugin.mkPlugin {
    inherit pkgs;
    name = "index";
  };

  # Declarative subagents rendered to a symlink-free `.claude/agents` directory.
  # Keep this outside the Claude plugin: plugins namespace subagent names, but
  # hooks and skills call these by bare `subagent_type` (`code-reviewer`, etc.).
  agentDefinitions = import (paths.packagesRoot + "/agent/subagents.nix") {
    inherit
      ix
      lib
      repoPackages
      ;
  };
  agentsDir = ix.agents.mkAgentsDir {
    inherit pkgs;
    agents = agentDefinitions.renderedAgents;
    inherit (agentDefinitions) rawFiles;
  };

  mcSource = ix.writeNushellApplication pkgs {
    name = "mc-source";
    text = builtins.readFile paths.tools.mcSource;
    runtimeInputs = [
      (pkgs.callPackage packageRegistry.byId.vineflower.path {inherit ix;})
    ];
    meta.description = "Decompile a Minecraft server jar with Mojang mappings via Vineflower";
  };

  updateSounds = ix.writeNushellApplication pkgs {
    name = "update-sounds";
    text = builtins.readFile paths.tools.updateSounds;
    meta.description = "Refresh the pinned Minecraft sound pack in packages/minecraft/sound";
  };

  benchFilesystem = import (paths.bench.filesystem + "/build.nix") {inherit ix pkgs;};

  # The indexbench CLI built for this system, fed to `mkBenchSuite` and the
  # `apps.bench` perf job. Also surfaced as `packages.indexbench` through the
  # registry; this binding just avoids re-resolving the package set here.
  inherit (repoPackages) indexbench;

  # The reproducible alloc-count bench binary from the shared workspace graph.
  # It installs the counting allocator and prints an `@bench name=allocations`
  # line, so its metric is deterministic and gateable as a flake check.
  indexbenchAllocDemo = ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "indexbench-alloc-demo";
    packageName = "indexbench";
    includeTestCases = false;
    meta.mainProgram = "indexbench-alloc-demo";
  };

  # The repo's own demonstration suite: a trivial macro command, run through the
  # framework end to end. `nix run .#bench` invokes this perf job. Consumers add
  # their own suites the same way via `ix.mkBenchSuite`. The `allocCheck` wires
  # the reproducible alloc-count bench into a flake check.
  indexbenchSelfDemo = ix.mkBenchSuite pkgs {
    name = "self-demo";
    inherit indexbench;
    macros = [
      {
        name = "true";
        command = "true";
      }
    ];
    allocCheck = {
      bench = lib.getExe indexbenchAllocDemo;
      # The demo makes exactly 64 heap allocations by construction (see
      # packages/indexbench/src/bin/alloc-demo.rs), so this budget is an exact,
      # toolchain-stable constant; any added allocation trips the gate.
      budgets.allocations = 64;
    };
  };

  # `paths.site` is the git-filtered `site` subtree input (a store copy, not
  # a local path), so `lib.fileset`/`gitTracked` cannot apply to it; the input
  # already scopes source identity to the subtree.
  siteSrc = paths.site;

  siteTests = ix.buildNpmVitest pkgs {
    pname = "ix-site";
    version = "0.1.0";
    src = siteSrc;
    preTest = ''
      node node_modules/@sveltejs/kit/src/cli.js sync
    '';
  };

  repoPackages = ix.packageSetFor pkgs;
  inherit (repoPackages) site;

  # De-forked patched sources + patch-DAG invariants exposed as
  # `checks.<system>.{patched-src,patch-dag}-<name>`. `patched-src-<name>` is the
  # seconds-fast "does the series still apply" gate, built from the same
  # `patchedSrc` the packages consume against the same raw upstream inputs, so it
  # can never drift from the real build. `patch-dag-<name>` is its textual
  # sibling, validating the committed `dag.json` against the pinned base. Both are
  # built by the shared `ix.mkForkChecks` (lib/mk-fork-checks.nix) — the one owner
  # of these check derivations, reused verbatim by ix for its own forks — driven
  # by index's fork-package list so a new entry there joins this set with no
  # change here. Each raw input is exposed on the `ix` handle as `<name>Src` (see
  # lib/default.nix sharedHelpers). Merged on every system, so
  # `nix build .#checks.aarch64-darwin.patch-dag-clippy` validates natively.
  forkChecks = ix.mkForkChecks {
    inherit pkgs;
    patchedSrcFor = ix.patchedSrcFor pkgs;
    inherit (ix) forkPackages;
    dagCheckSrc = ix.forkDagCheckSrc;
    forkSrcInputs = lib.genAttrs (map (fork: fork.name) ix.forkPackages) (
      name: ix."${name}Src"
    );
    patchesRoot = paths.root;
    flakeLock = lib.importJSON (paths.root + "/flake.lock");
  };

  # Per-attempt-patch closure build gates (RFC 0010 A3, #2098): for each fork
  # opted in via `closureGates = true` in lib/fork-packages.nix, the fork
  # package rebuilt with its series restricted to each attempt-marked patch's
  # dag.json closure -- exactly the standalone series `upstream-pr` ships
  # upstream, so a red gate means the upstream PR would be broken. The gate
  # derivations live on the opted-in package's `passthru.closureGates` (the
  # package owns its own re-instantiation; see packages/nix/nix/default.nix);
  # this map only keys them by fork name so the scheduled fork-closure-gates
  # workflow and the `upstream-sync --open` preflight can `nix eval` the set
  # and `nix build .#forkClosureGates.<system>.<fork>."<patch>"`. NEVER merged
  # into `checks`/`ciChecks`: these are heavy full-package builds, and per-PR
  # flake-check cost must stay flat (the attrset is lazy, so enumerating it
  # forces nothing heavy).
  forkClosureGates = let
    # Fork name -> the repo package carrying that fork's gates. A fork flagged
    # `closureGates = true` without an entry here fails eval loudly instead of
    # silently publishing no gates.
    gatePackages = {
      nix = repoPackages.nix-ix;
    };
  in
    lib.genAttrs' (lib.filter (fork: fork.closureGates or false) ix.forkPackages) (
      fork:
        lib.nameValuePair fork.name
        (gatePackages.${fork.name}
          or (throw "lib/per-system.nix: fork `${fork.name}` sets closureGates = true in lib/fork-packages.nix but gatePackages maps no package for it"))
        .closureGates
    );

  # One general updater for every content source in the repo, run in parallel
  # via dag-runner (the same engine `lint` uses). The Minecraft catalog and
  # sound updaters are fixed apps; the pinned prebuilt-binary updaters
  # (claude-code, yc, ...) are discovered from the registry `updateScript` flag,
  # so adding such a package joins this set with no change here. The nodes are
  # independent (each writes its own source files: mod/loader/sound catalogs or
  # packages/<id>/manifest.json), so they run concurrently. dag-runner fails the
  # run if any node exits non-zero, so a bad signature or fetch error surfaces
  # in CI. Each updater writes relative to the repo root, so `update` must run
  # from the repo root.
  updatableEntries = packageRegistry.updateScriptEntriesFor system;
  updaterFor = entry: let
    pkg =
      lib.attrByPath entry.packageSet.attrPath
      (throw "update: package `${entry.id}` is flagged `updateScript = true` but is absent from the package set for ${system}")
      repoPackages;
  in
    lib.getExe (
      pkg.updateScript
        or (throw "update: package `${entry.id}` is flagged `updateScript = true` but exposes no `passthru.updateScript`")
    );
  updateNodes =
    {
      mods.command = [(lib.getExe updateMods)];
      loaders.command = [(lib.getExe updateLoaders)];
      sounds.command = [(lib.getExe updateSounds)];
    }
    // lib.genAttrs' updatableEntries (
      entry: lib.nameValuePair entry.id {command = [(updaterFor entry)];}
    );
  updateSpec = (pkgs.formats.json {}).generate "update-dag.json" {nodes = updateNodes;};
  # Machine-readable registry view for update.yml's "Build changed packages"
  # step: repo-relative package directory -> the flake attr that builds it on
  # this system. The workflow maps each file the updaters changed to its owning
  # package through this table instead of guessing an attr from path segments,
  # which breaks for nested catalog manifests (#2036). Restricted to entries
  # with a `flake` target enabled here, so a platform-gated updater (dia is
  # aarch64-darwin-only) is absent from the Linux map and gets skipped rather
  # than built as a missing attr.
  updatablePackages = lib.genAttrs' (
    lib.filter (entry: entry.updateScript) (packageRegistry.flakeEntriesFor system)
  ) (entry: lib.nameValuePair "packages/${entry.relativePath}" entry.flake.attrName);
  update = ix.writeRustApplication pkgs {
    name = "update";
    meta.description = "Refresh every repo content source (Minecraft catalogs + pinned binaries) in parallel via dag-runner";
    text = ''
      //! Exec dag-runner over the generated update DAG spec.
      use std::os::unix::process::CommandExt;

      fn main() {
          let err = std::process::Command::new("${lib.getExe repoPackages.dag-runner}")
              .args(std::env::args_os().skip(1))
              .arg("${updateSpec}")
              .exec();
          eprintln!("update: exec dag-runner failed: {err}");
          std::process::exit(1);
      }
    '';
  };

  # Cross-compiled standalone packages, exposed as
  # `packages.<host>.<attr>-<triple>` and optionally aliased into native Darwin
  # package namespaces by flake.nix. Linux-only: the Apple (zig + macOS SDK) and
  # Rust target graph run on a Linux build host; Darwin hosts build native
  # packages directly and cannot host this Linux→Darwin lane. Package definitions
  # stay target-agnostic: the cross lane swaps the `ix.rustWorkspace.units`
  # handle underneath them instead of passing a separate cross API.
  darwinTargetsBySystem = {
    aarch64-darwin = "aarch64-apple-darwin";
    x86_64-darwin = "x86_64-apple-darwin";
  };
  targetSystemFor = target:
    if lib.hasSuffix "-apple-darwin" target
    then
      if lib.hasPrefix "aarch64-" target
      then "aarch64-darwin"
      else "x86_64-darwin"
    else throw "cross: unsupported target `${target}`";
  crossEntries = packageRegistry.crossEntriesFor system;
  crossWorkspace = ix.rustWorkspaceFor pkgs;
  crossIxFor = target: let
    targetWorkspace =
      crossWorkspace
      // {
        units = crossWorkspace.unitsFor {inherit target;};
      };
  in
    ix
    // {
      inherit pkgs;
      cargoUnit = ix.cargoUnitFor pkgs;
      rustWorkspace = targetWorkspace;
      cross = {
        isCross = true;
        inherit target;
        targetSystem = targetSystemFor target;
      };
      wrapPackage = wrapperPkgs: args: ix.wrapPackage wrapperPkgs (args // {isCross = true;});
    };
  buildCrossPackage = target: entry:
    lib.callPackageWith (
      pkgs
      // {
        inherit entry repoPackages;
        ix = crossIxFor target;
        writeNushellApplication = ix.writeNushellApplication pkgs;
        updateScriptWriter = ix.writeNushellApplication pkgs;
      }
    )
    entry.path {};
  crossPackages = lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux (
    lib.listToAttrs (
      lib.concatMap (
        entry:
          map (
            target: lib.nameValuePair "${entry.cross.attrName}-${target}" (buildCrossPackage target entry)
          )
          entry.cross.targets
      )
      crossEntries
    )
  );
  # The eval-time IFD closure of each cross target's unit graph. A Mac cannot
  # *build* a Linux→Darwin cross output, but the Darwin package aliases force it
  # to *evaluate* the cross derivation, and that eval imports the rendered
  # `cargo-units.nix` (which is generated from `cargo-unit-graph.json`, itself
  # generated from the vendor dir). Those three are build-time deps of the cross
  # outputs, so `attic push` of the outputs' *runtime* closures never carries
  # them (RFC 0009's substitute-or-nothing trap: #1687). Publishing them lets a
  # Mac substitute the IFD outputs instead of trying to build x86_64-linux drvs
  # at eval; because these are input-addressed drvs, their eval-time out paths
  # are known, so cache-push's probe sees the same paths a Mac's eval demands.
  # Keyed by distinct cross target (the unit graph is shared per target, not per
  # package), derived from `crossEntries` so a new cross target or entry joins
  # this set with no hand-kept list. Same Linux-host gate as `crossPackages`:
  # the cross graphs only build on the Linux host that owns the cross lane.
  crossIfdRoots = lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux (
    let
      crossTargets = lib.unique (lib.concatMap (entry: entry.cross.targets) crossEntries);
      rootsForTarget = target: let
        units = crossWorkspace.unitsFor {inherit target;};
      in
        # These three ARE the whole eval-time closure: the `import unitsNix`
        # forces `unitsNix`, which references only `unitGraphJson` and `vendorDir`
        # (the cargo-lock it also reads is a plain flake source path, always
        # present). `cargo-vendor-config.toml` is not a fourth root: it is a
        # build input of the `unitGraphJson` builder, not on the import path and
        # not in `vendorDir`'s closure, so substituting `unitGraphJson`'s output
        # makes it moot -- the Mac never runs that builder.
        {
          "cross-ifd-${target}-units-nix" = units.unitsNix;
          "cross-ifd-${target}-unit-graph" = units.unitGraphJson;
          "cross-ifd-${target}-vendor-dir" = units.vendorDir;
        };
    in
      lib.mergeAttrsList (map rootsForTarget crossTargets)
  );
  # A cross package whose build rides a distinct `cargoUnit.buildWorkspace`
  # instead of the shared `crossWorkspace` (codex: its codex-rs is a second
  # workspace) exposes that workspace's unit-graph IFD artifacts via
  # `passthru.workspaceIfdRoots`. `crossIfdRoots` only covers the shared
  # workspace, so harvest these too -- otherwise a Mac consumer substituting the
  # cross output re-vendors/re-renders that graph at eval and hits the #1890
  # trap on x86_64-linux drvs it cannot build. Generic over `crossPackages`, so
  # a future second-workspace cross package joins with no hand-kept list.
  crossPackageIfdRoots = lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux (
    lib.concatMapAttrs (
      name: pkg:
        lib.mapAttrs' (
          rootName: drv: lib.nameValuePair "cross-ifd-${name}-${rootName}" drv
        )
        (pkg.passthru.workspaceIfdRoots or {})
    )
    crossPackages
  );
  darwinPackageAliases = lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux (
    lib.genAttrs (lib.attrNames darwinTargetsBySystem) (
      darwinSystem: let
        target = darwinTargetsBySystem.${darwinSystem};
      in
        lib.listToAttrs (
          lib.concatMap (
            entry:
              lib.optional (entry.cross.exposeNativeDarwin && builtins.elem target entry.cross.targets) (
                lib.nameValuePair entry.cross.attrName crossPackages."${entry.cross.attrName}-${target}"
              )
          )
          crossEntries
        )
    )
  );

  repoFlakePackages = lib.genAttrs' (packageRegistry.flakeEntriesFor system) (
    entry:
      lib.nameValuePair entry.flake.attrName (
        lib.attrByPath entry.packageSet.attrPath
        (throw "packages/${entry.relativePath}/package.nix: flake output `${entry.flake.attrName}` needs packageSet.attrPath")
        repoPackages
      )
  );

  rustPackageTestSets = let
    cargoUnit = ix.cargoUnitFor pkgs;
    rustWorkspace = ix.rustWorkspaceFor pkgs;
    # A crate with a `packageSet` is built through `repoPackages` and carries
    # its own `passthru.tests`. A lib-only workspace crate has no `packageSet`
    # and is not in `repoPackages`, so select its library straight from the
    # shared unit graph (same path ix-vt's default.nix uses). The library unit
    # key is the Cargo package name with dashes underscored.
    packageTestsFor = entry:
      if entry.packageSet != null
      then
        (
          lib.attrByPath entry.packageSet.attrPath
          (throw "packages/${entry.relativePath}/package.nix: passthruTests needs packageSet.attrPath")
          repoPackages
        ).passthru.tests or {
        }
      else
        (cargoUnit.selectLibraryWithTests rustWorkspace.units {
          library = lib.replaceStrings ["-"] ["_"] entry.id;
          packageName = entry.id;
        }).passthru.tests or {
        };
    # Two keyings of the same leaf test derivations:
    #
    #  * `flat` keys each per-#[test] check as its own top-level name
    #    (`<prefix>-<target>-tests-<case>`). This is what the public `checks`
    #    output needs: the flake schema requires every `checks.<system>.<name>`
    #    to be a derivation, so a nested attrset there fails `nix flake check`.
    #
    #  * `sharded` nests each package's checks under one `recurseForDerivations`
    #    attr (`<prefix>.<target>-tests-<case>`). This is what the memory-bounded
    #    CI evaluator (nix-fast-build / nix-eval-jobs / blast-radius) consumes
    #    through the separate `ciChecks` output.
    #
    # Why the sharded shape exists: nix-eval-jobs hands the root attrpath to one
    # worker and forces its child names to recurse. With the flat set, that one
    # worker forces every crate's per-#[test] manifest IFD at once and balloons
    # to tens of GiB, which earlyoom kills on the shared CI host. The nested
    # shape makes the root return cheap per-package names and forces each
    # crate's manifests inside its own worker job, which restarts at the memory
    # cap between packages (ENG-2201). The nested value must stay a thunk:
    # filtering empties (e.g. `tests != {}`) would force every manifest during
    # enumeration and reintroduce the balloon, so empty groups are left in.
    flatPackageChecks = prefix: tests: lib.mapAttrs' (n: t: lib.nameValuePair "${prefix}-${n}" t) tests;
    shardedPackageChecks = prefix: tests: {
      ${prefix} =
        tests
        // {
          recurseForDerivations = true;
        };
    };
    repoEntries = packageRegistry.passthruTestEntriesFor system;
    moduleRustPackages = {
      resource-monitor-stats-writer = cargoUnit.selectBinaryWithTests rustWorkspace.units {
        binary = "resource-monitor-stats-writer";
      };
    };
    # cargoAudit scans the single workspace Cargo.lock against the advisory DB,
    # so it is one lockfile-scoped check (it rebuilds only on a Cargo.lock
    # change, never on a source edit) rather than a per-crate gate. Expose it
    # once instead of aliasing the same derivation onto every crate.
    workspaceAuditTests = lib.optionalAttrs (rustWorkspace.units.policyChecks ? cargoAudit) {
      rust-cargoAudit = rustWorkspace.units.policyChecks.cargoAudit;
    };
    collectRust = group:
      lib.mergeAttrsList (
        map (entry: group entry.passthruTests.prefix (packageTestsFor entry)) repoEntries
        ++ lib.mapAttrsToList (
          packageName: package: group "rust-${packageName}" (package.passthru.tests or {})
        )
        moduleRustPackages
      )
      // workspaceAuditTests;
  in {
    flat = collectRust flatPackageChecks;
    sharded = collectRust shardedPackageChecks;
  };

  lintSource = fs.toSource {
    inherit (paths) root;
    fileset = fs.gitTracked paths.root;
  };

  # Just the astlog rules file plus its fixture pairs, so the rules self-test
  # below only rebuilds when the rules or fixtures change, not on every
  # tracked-file edit the way `lintSource` does.
  astlogRulesSource = fs.toSource {
    inherit (paths) root;
    fileset = fs.intersection (fs.gitTracked paths.root) (paths.root + "/astlog-rules");
  };

  andrewZellij = import (paths.root + "/users/andrewgazelka/config/zellij") {
    configRoot = paths.root + "/users/andrewgazelka/config";
    inherit (pkgs) lib stdenvNoCC zellijPlugins;
    xdgConfigHome = "/Users/andrewgazelka/.config";
  };
  andrewZellijConfig = pkgs.writeText "andrewgazelka-zellij.kdl" (ix.kdl.render andrewZellij.settings);

  tests = import paths.tests {
    inherit
      nixpkgs
      ix
      paths
      home-manager
      ;
  };

  exampleFleets = ix.exampleFleetsFor {hostSystem = system;};

  # Same fleets with "health-check-" prepended to every external name, so the
  # lifecycle scripts that force-delete VMs by name can never clobber an
  # unrelated production VM that happens to share the example's node name
  # (`nginx`, `factions`, ...). `withNodePrefix` only rewrites plan data, so
  # both surfaces share one NixOS closure evaluation per node instead of
  # evaluating every example fleet twice (ENG-2411).
  healthCheckExampleFleets =
    lib.mapAttrs (
      _name: fleet: fleet.withNodePrefix "health-check-"
    )
    exampleFleets;

  # Surface every example's `ix fleet <sub>` wrapper as a flake package.
  # Each example contributes `packages.<system>.<example>-{up,health,...}`,
  # which lets `nix run .#nginx-lifecycle-up` invoke the existing fleet
  # plumbing through the wrapper's `meta.mainProgram`, and
  # `nix build .#nginx-lifecycle-up` produce the wrapper script on disk.
  examplePackages = let
    fleetSubs = [
      "up"
      "health"
      "status"
      "logs"
      "replace"
      "switch"
      "diff"
    ];
  in
    lib.concatMapAttrs (
      name: fleet:
        lib.genAttrs' fleetSubs (sub: {
          name = "${name}-${sub}";
          value = fleet.${sub}.overrideAttrs (old: {
            meta =
              (old.meta or {})
              // {
                description = "Run `ix fleet ${sub}` against the ${name} example fleet";
              };
          });
        })
    )
    exampleFleets;

  healthChecks =
    import ./image/health-checks.nix
    {
      inherit lib pkgs;
      inherit (ix) kdl writeNushellApplication;
      dagRunner = repoPackages.dag-runner;
    }
    {
      exampleFleets = healthCheckExampleFleets;
      exampleNames = lib.attrNames exampleFleets;
    };

  baseImage = ix.mkImage {
    modules = [(paths.root + "/images/system/base")];
  };

  vcfsGuestEvalImage = ix.mkImage {
    modules = [(paths.root + "/images/system/vcfs-guest-eval")];
  };

  # Non-NixOS OCI example images (ubuntu, debian, ...). They live under
  # `examples/oci` with the same hierarchical shape as fleet examples, but
  # return images instead of fleet plans and are exposed as opt-in packages only.
  nonNixExampleImages =
    lib.mapAttrs'
    (
      name: entry:
        lib.nameValuePair "non-nix-${name}" (
          import (entry.path + "/ix.nix") {
            index = {
              lib = ix;
            };
          }
        )
    )
    (
      ix.discoverTree {
        root = paths.examples + "/oci";
        requiredFiles = ["ix.nix"];
      }
    );

  # The content-addressed `image.json` for each non-Nix example, surfaced as its
  # own package so the small artifact is buildable directly (`nix build
  # .#non-nix-ubuntu-description`) and cached independently of the materialized
  # tar it regenerates. See #679.
  nonNixExampleDescriptions =
    lib.mapAttrs' (
      name: image: lib.nameValuePair "${name}-description" image.passthru.description
    )
    nonNixExampleImages;

  # Build the check catalog from a rust-package keying. `checks` (flat: one
  # derivation per `checks.<system>.<name>`, required by the flake schema and
  # `nix flake check`) and `ciChecks` (sharded: one `recurseForDerivations` group
  # per package, what the memory-bounded CI evaluator consumes) share the same
  # explicit checks; only the rust keying differs (ENG-2201). The
  # collision guard runs per keying, so producing `ciChecks` only forces the
  # cheap per-package names, never the flat per-#[test] spine.
  catalogFor = rustPackageSet:
    lib.optionalAttrs (system == ix.system) (
      let
        rustChecks =
          {
            cargo-unit-real-workspaces = tests.cargoUnitRealWorkspaces;
            cargo-unit-prebuilt-library = tests.cargoUnitPrebuiltLibrary;
            sdk-rust-prebuilt = tests.sdkRustPrebuilt;
            # Strict zuban + ruff ANN gate over the public ix-sdk Python sources
            # (ENG-3131); the SDK is setuptools-built, so this is its build-time
            # enforcement in place of a buildUvApplication pyChecker flag.
            sdk-python-strict = tests.sdkPythonStrict;
          }
          // rustPackageSet;
        explicitChecks = {
          inherit (tests) eval;
          # Boots a NixOS VM running the minecraft-blocks producer's Paper
          # server and asserts the BlockEvents plugin's onEnable succeeded
          # with no exception (ENG-2186). Paper's paperclip bootstrap is
          # pre-run at build time so the VM never needs the network; see
          # tests/minecraft-blocks-vm.nix.
          minecraft-blocks-vm = tests.minecraftBlocksVm;
          # Boots a NixOS VM running the Minestom spleef example server under
          # `services.minestom` and asserts it serves the Minecraft protocol
          # (readiness log line, open port, real server-list ping); see
          # tests/minestom-spleef-vm.nix.
          minestom-spleef-vm = tests.minestomSpleefVm;
          # Builds the base OCI archive and asserts its baked nix store DB
          # registers the pinned nixpkgs source as valid, so a fresh VM's first
          # `nix` command does not re-copy the tree through VCFS (ix
          # #1748/#1749/#1815). Its own check because it builds an image.
          base-image-nix-db = tests.baseImageNixDb;
          # Skills and subagents are rendered live by the SessionStart hook.
          # This gate forces both materialized directories to build.
          agent-skills = pkgs.runCommand "agent-skills-check" {} ''
            test -d ${skillsDir}
            test -d ${agentsDir}
            mkdir -p "$out"
          '';
          # Pins the last-applied 3-way merge behind homeModules.mutable-json:
          # first-install, preserve an app-written key, enforce a key the app
          # changed, prune a key Nix stopped declaring, and keep a sibling key
          # while a declared array is replaced atomically.
          mutable-json-merge =
            pkgs.runCommand "mutable-json-merge-check" {nativeBuildInputs = [pkgs.jq];}
            ''
              prog=${ix.mutableJson.mergeProgram}
              run() { jq -ncS --argjson last "$1" --argjson live "$2" --argjson new "$3" -f "$prog"; }
              check() {
                expected=$(printf '%s' "$2" | jq -cS .)
                if [ "$expected" != "$3" ]; then
                  echo "FAIL $1: expected $expected got $3" >&2
                  exit 1
                fi
                echo "ok $1"
              }
              check first-install '{"permissions":{"defaultMode":"bypass"}}' \
                "$(run '{}' '{}' '{"permissions":{"defaultMode":"bypass"}}')"
              check preserve-app-key '{"permissions":{"defaultMode":"bypass"},"theme":"dark"}' \
                "$(run '{"permissions":{"defaultMode":"bypass"}}' '{"permissions":{"defaultMode":"bypass"},"theme":"dark"}' '{"permissions":{"defaultMode":"bypass"}}')"
              check enforce-changed '{"permissions":{"defaultMode":"bypass"},"theme":"dark"}' \
                "$(run '{"permissions":{"defaultMode":"bypass"}}' '{"permissions":{"defaultMode":"off"},"theme":"dark"}' '{"permissions":{"defaultMode":"bypass"}}')"
              check prune-dropped '{"a":1,"c":3}' \
                "$(run '{"a":1,"b":2}' '{"a":1,"b":2,"c":3}' '{"a":1}')"
              check nested-atomic-array '{"p":{"allow":["x"]},"t":1}' \
                "$(run '{"p":{"allow":["x"]}}' '{"p":{"allow":["x","y"]},"t":1}' '{"p":{"allow":["x"]}}')"
              # Divergent live shape at a path we stop declaring must not abort:
              # the app replaced object `permissions` with a scalar, Nix dropped it.
              check divergent-live-shape '{"permissions":"all"}' \
                "$(run '{"permissions":{"defaultMode":"x"}}' '{"permissions":"all"}' '{}')"
              mkdir -p "$out"
            '';
          # Pins the formatProvenance seam (lib/util/format-provenance.nix): a
          # wrapped comment-capable generator must emit the `# generated by
          # <file>:<line>` header as line 1 and keep the format's own rendering
          # intact below it, for both an unsafeGetAttrPos-shaped position and
          # an explicit { file, line } pair.
          format-provenance = let
            sample = {greeting = "hello";};
            tomlFile =
              (ix.formatProvenance.withHeader pkgs {
                format = pkgs.formats.toml {};
                position = builtins.unsafeGetAttrPos "greeting" sample;
              }).generate "sample.toml"
              sample;
            kvFile = (ix.formatProvenance.withHeader pkgs {
              format = pkgs.formats.keyValue {};
              position = {
                file = "lib/per-system.nix";
                line = 1;
              };
            }).generate "sample.env" {GREETING = "hello";};
          in
            pkgs.runCommand "format-provenance-check" {} ''
              head -n1 ${tomlFile} | grep -Eqx '# generated by .+/per-system\.nix:[0-9]+'
              grep -qx 'greeting = "hello"' ${tomlFile}
              head -n1 ${kvFile} | grep -qx '# generated by lib/per-system\.nix:1'
              grep -qx 'GREETING=hello' ${kvFile}
              mkdir -p "$out"
            '';
          # Offline schema gate for the loader manifests. `deepSeq` forces
          # every Paper / Velocity / Fabric per-version lock through
          # `readLoaderManifest` in `lib/artifacts.nix`, so malformed JSON or a
          # missing key fires here before any image starts evaluating. The
          # forced surface is the parsed-and-validated manifest data, not the
          # wrapped `fetchurl` derivations, to keep this check pure eval.
          loader-manifests = let
            forced = builtins.deepSeq ix.artifacts.minecraft.loaderManifests "ok";
          in
            pkgs.runCommand "loader-manifests-check" {} ''
              printf '%s\n' '${forced}' > "$out"
            '';
          # Rule self-test for the astlog lint rules (nix.astlog + rust.astlog):
          # every (lint ...) declaration must have a committed fixture pair and
          # fire exactly on the violating one, driven through the same `astlog
          # scan --json` surface the lint gate uses. A lint that never fires in
          # tests is unproven (its query may have silently stopped matching), so
          # a missing or non-firing fixture fails the build, as does a rule
          # without a lint declaration (it would silently drop out of the gate).
          # Fixtures are stored as `.fixture` (not `.nix`/`.rs`) so the repo lint
          # stages (alejandra / statix / deadnix / astlog itself) never scan the
          # deliberately-violating snippets; the check stages each back to its
          # ruleset's extension (`.nix` for nix.astlog, `.rs` for rust.astlog —
          # astlog selects the grammar by file extension) before running the
          # binary. `scan` exits nonzero on the violating fixture by design, so
          # the jq pipelines deliberately take the JSON regardless of exit code.
          astlog-rules =
            pkgs.runCommand "astlog-rules-check"
            {
              nativeBuildInputs = [
                repoPackages.astlog
                pkgs.jq
              ];
            }
            ''
              root=${astlogRulesSource}/astlog-rules
              tests="$root/tests"
              fail=0
              # Each ruleset paired with the source extension its fixtures take.
              check_ruleset() {
                rules="$1"
                ext="$2"
                # Rules without a (lint ...) are legitimate helper relations
                # (joins/negation need intermediate relations), so they are not
                # required to back a lint. The meaningful checks remain: every
                # lint has a good/bad fixture pair that fires/stays-clean, and
                # every fixture dir backs some lint. `astlog` itself rejects a
                # lint that names an undefined relation at parse time.
                for rule in $(sed -n 's/^(lint \([a-z0-9-]*\).*/\1/p' "$rules" | sort -u); do
                  dir="$tests/$rule"
                  if [ ! -f "$dir/bad.fixture" ] || [ ! -f "$dir/good.fixture" ]; then
                    echo "lint $rule has no fixture pair under astlog-rules/tests/$rule" >&2
                    fail=1
                    continue
                  fi
                  work=$(mktemp -d)
                  cp "$dir/bad.fixture" "$work/bad.$ext"
                  cp "$dir/good.fixture" "$work/good.$ext"
                  # `astlog scan` exits nonzero on a violating fixture by
                  # design; capture its JSON (`|| true` so the by-design exit
                  # does not abort the `set -o pipefail` build) and count
                  # separately, rather than piping straight into jq.
                  bad_json=$(astlog scan "$rules" "$work/bad.$ext" --json || true)
                  good_json=$(astlog scan "$rules" "$work/good.$ext" --json || true)
                  bad=$(jq --arg r "$rule" '[.[] | select(.rule == $r)] | length' <<<"$bad_json")
                  good=$(jq --arg r "$rule" '[.[] | select(.rule == $r)] | length' <<<"$good_json")
                  if [ "$bad" = 0 ]; then
                    echo "lint $rule did not fire on its violating fixture" >&2
                    fail=1
                  fi
                  if [ "$good" != 0 ]; then
                    echo "lint $rule fired $good finding(s) on its valid fixture" >&2
                    fail=1
                  fi
                done
              }
              check_ruleset "$root/nix.astlog" nix
              check_ruleset "$root/rust.astlog" rs
              check_ruleset "$root/cargo.astlog" toml
              check_ruleset "$root/elixir.astlog" ex
              # Every fixture dir must back a lint in one of the rulesets.
              for dir in "$tests"/*/; do
                rule=$(basename "$dir")
                if ! grep -q "^(lint $rule " "$root/nix.astlog" "$root/rust.astlog" "$root/cargo.astlog" "$root/elixir.astlog"; then
                  echo "fixture dir astlog-rules/tests/$rule matches no lint" >&2
                  fail=1
                fi
              done
              if [ "$fail" != 0 ]; then
                exit 1
              fi
              mkdir -p "$out"
            '';
          # End-to-end proof that scipql resolves SCIP monikers and acts only on
          # the right symbol, exercising all three surfaces (query / fix /
          # rename) of the real pipeline. The wrapped CLI bakes rust-analyzer +
          # the pinned toolchain + souffle; the fixture is a dependency-free
          # crate with a `net::Socket` and a same-named `mock::Socket`, so
          # rust-analyzer's `cargo metadata` needs no network. Tree-sitter
          # (astlog) could not tell the two `Socket`s apart; this is the
          # semantic-disambiguation guarantee.
          scipql-e2e =
            pkgs.runCommand "scipql-e2e-check"
            {
              nativeBuildInputs = [repoPackages.scipql];
            }
            ''
              export HOME="$TMPDIR/home"
              mkdir -p "$HOME"
              cp -r ${
                builtins.path {
                  name = "scipql-two-sockets-fixture";
                  path = paths.packagesRoot + "/code/scipql/tests/fixtures/two-sockets";
                }
              } work
              chmod -R u+w work
              cd work
              fail=0

              scipql index . -o index.scip

              # query: the two same-named structs resolve to distinct monikers.
              # (printf, not a heredoc: a heredoc terminator would not sit at
              # column 0 after Nix strips the indented string's indentation.)
              printf '%s\n' \
                '.decl sockets(sym:symbol)' \
                '.output sockets' \
                'sockets(s) :- occurrence(s, _, _, _, "definition"), symbol_info(s, _, "Socket").' \
                > sockets.dl
              q=$(scipql query index.scip sockets.dl)
              echo "$q" | grep -q 'net/Socket#' || { echo "query: missing net/Socket# definition" >&2; fail=1; }
              echo "$q" | grep -q 'mock/Socket#' || { echo "query: missing mock/Socket# definition" >&2; fail=1; }

              # fix: the replacement text is COMPUTED in datalog (cat + a join to
              # the display name), not a constant, and still scoped to net by moniker.
              printf '%s\n' \
                'edit(path, start, end, cat("Net", name)) :-' \
                '  occurrence(sym, path, start, end, _),' \
                '  symbol_info(sym, _, name),' \
                '  substr(sym, strlen(sym) - strlen("net/Socket#"), strlen("net/Socket#")) = "net/Socket#".' \
                > netname.dl
              d=$(scipql fix index.scip netname.dl)
              echo "$d" | grep -q 'NetSocket' || { echo "fix: datalog-computed replacement (cat) did not apply" >&2; fail=1; }
              echo "$d" | grep -q 'src/mock.rs' && { echo "fix: computed edit wrongly touched mock.rs" >&2; fail=1; }

              # rename: apply to disk, then assert the net struct + its reference
              # changed while mock::Socket and the net struct's own fd field did not.
              scipql rename index.scip 'net/Socket#' Stream --write
              grep -q 'pub struct Stream' src/net.rs || { echo "rename: net::Socket was not renamed" >&2; fail=1; }
              grep -q 'net::Stream' src/lib.rs || { echo "rename: the net::Socket reference was not renamed" >&2; fail=1; }
              grep -q 'pub struct Socket' src/mock.rs || { echo "rename: mock::Socket was wrongly changed" >&2; fail=1; }
              grep -q 'pub fd: i32' src/net.rs || { echo "rename: the struct's own fd field was wrongly renamed" >&2; fail=1; }

              if [ "$fail" != 0 ]; then
                echo "--- net.rs ---" >&2; cat src/net.rs >&2
                echo "--- mock.rs ---" >&2; cat src/mock.rs >&2
                echo "--- lib.rs ---" >&2; cat src/lib.rs >&2
                exit 1
              fi
              mkdir -p "$out"
            '';
          run-records-session = repoPackages.run.passthru.tests.recordsSession;
          # hive's quality lane through the same shared ix.buildElixirCheck:
          # `mix compile --warnings-as-errors` (Elixir 1.18's set-theoretic type
          # checker) plus format, `mix credo --strict`, and test. The lint half
          # is also astlog-rules/elixir.astlog. See
          # packages/andrewgazelka/hive/default.nix.
          hive-elixir = repoPackages.hive.passthru.tests.elixir;
          # Deterministic alloc-count gate for indexbench: runs the counting-
          # allocator demo bench once through `indexbench assert` and fails if its
          # allocation count exceeds the declared budget. Reproducible, unlike
          # timing/RSS, so it earns a flake check; the timing/RSS perf job lives
          # under `apps.bench` instead.
          indexbench-self-demo-alloc = indexbenchSelfDemo.check;
          lint = pkgs.runCommand "ix-lint" {nativeBuildInputs = [pkgs.coreutils];} ''
            cp -R ${lintSource} source
            chmod -R u+w source
            cd source
            ${lib.getExe repoPackages.lint}
            mkdir -p "$out"
          '';
          filename-policy =
            pkgs.runCommand "filename-policy-check"
            {
              nativeBuildInputs = [pkgs.coreutils];
            }
            ''
              mkdir source
              cd source
              touch repository-config.json zellij-layout.kdl
              if ${lib.getExe repoPackages.lint.passthru.lintStage} filenames >output 2>&1; then
                echo "filename policy accepted repository-config.json" >&2
                exit 1
              fi
              grep -F "repository-config.json" output
              grep -F "zellij-layout.kdl" output
              touch "$out"
            '';
          # Both halves of the dirnames stage: a marker-less doubled segment is
          # flagged, an eponym package root (package.nix) is exempt.
          dirname-policy =
            pkgs.runCommand "dirname-policy-check"
            {
              nativeBuildInputs = [pkgs.coreutils];
            }
            ''
              mkdir source
              cd source
              mkdir -p packages/foo/foo packages/bar/bar
              touch packages/bar/bar/package.nix
              if ${lib.getExe repoPackages.lint.passthru.lintStage} dirnames >output 2>&1; then
                echo "dirname policy accepted packages/foo/foo" >&2
                exit 1
              fi
              grep -F "packages/foo/foo" output
              if grep -F "packages/bar/bar" output; then
                echo "dirname policy exempted nothing: flagged the eponym package packages/bar/bar" >&2
                exit 1
              fi
              touch "$out"
            '';
          zellij-config = pkgs.runCommand "zellij-config-check" {nativeBuildInputs = [pkgs.zellij];} ''
            export HOME="$TMPDIR/home"
            mkdir -p "$HOME" "$out"
            zellij --config ${andrewZellijConfig} setup --check >"$out/check.txt"
          '';
          # Exercises the trusted half of the blast-radius PR comment: the
          # validate/render jq embedded in its workflow, extracted from the YAML so
          # the test can't drift from what the trusted comment job runs. The
          # report-building logic lives in the `blast-radius` Rust crate and is
          # covered by its own unit tests. See packages/blast-radius/tests/blast-radius-test.sh.
          blast-radius-test =
            pkgs.runCommand "blast-radius-test"
            {
              nativeBuildInputs = [
                pkgs.bash
                pkgs.coreutils
                pkgs.diffutils
                pkgs.jq
                pkgs.yq-go
              ];
            }
            ''
              cp -R ${lintSource} source
              chmod -R u+w source
              cd source
              export HOME="$TMPDIR/home"
              mkdir -p "$HOME"
              bash packages/blast-radius/tests/blast-radius-test.sh
              mkdir -p "$out"
            '';
          # Proves the Linux→macOS cross toolchain actually emits a Darwin object,
          # which a successful build alone does not assert. `file` reads the Mach-O
          # header; a regression in the zig/SDK wiring fails here on x86_64-linux CI
          # rather than silently shipping a wrong-arch binary.
          cross-darwin-smoke = pkgs.runCommand "cross-darwin-smoke" {nativeBuildInputs = [pkgs.file];} ''
            bin=${crossPackages.dag-runner-aarch64-apple-darwin}/bin/dag-runner
            info=$(file -b "$bin")
            echo "$info"
            case "$info" in
              *Mach-O*arm64*) ;;
              *)
                echo "expected Mach-O arm64, got: $info" >&2
                exit 1
                ;;
            esac
            mkdir -p "$out"
          '';
          cross-darwin-web-monitor-smoke =
            pkgs.runCommand "cross-darwin-web-monitor-smoke" {nativeBuildInputs = [pkgs.file];}
            ''
              pkg=${crossPackages.nix-web-monitor-aarch64-apple-darwin}
              bin=$pkg/bin/.nix-web-monitor-unwrapped
              info=$(file -b "$bin")
              echo "$info"
              case "$info" in
                *Mach-O*arm64*) ;;
                *)
                  echo "expected Mach-O arm64, got: $info" >&2
                  exit 1
                  ;;
              esac
              read -r shebang < "$pkg/bin/nix-web-monitor"
              case "$shebang" in
                "#!/bin/sh") ;;
                *)
                  echo "expected /bin/sh wrapper, got: $shebang" >&2
                  exit 1
                  ;;
              esac
              test -f "$pkg/share/nix-web-monitor/index.html"
              mkdir -p "$out"
            '';
          site-test = siteTests.all;
        };
        checkNameCollisions = lib.intersectLists (lib.attrNames explicitChecks) (lib.attrNames rustChecks);
      in
        assert lib.assertMsg (checkNameCollisions == [])
        "checks: duplicate names across explicit/rust sets: ${lib.concatStringsSep ", " checkNameCollisions}";
          explicitChecks // rustChecks
    );
  packageSet =
    lib.optionalAttrs (system == ix.system) {
      base = baseImage;
      vcfs-guest-eval = vcfsGuestEvalImage;
    }
    // {
      health-checks = healthChecks.dag;
      health-checks-zellij = healthChecks.zellij;
      inherit site;
      site-dev = site.passthru.devServer;
      bench-filesystem = benchFilesystem;
      update-mods = updateMods;
      update-loaders = updateLoaders;
      inherit update;
      ix-shell-sync-ignored = ixShellSyncIgnored;
      mc-source = mcSource;
      update-sounds = updateSounds;
      agents = agentsDir;
      skills = skillsDir;
      claude-plugin = claudePluginDir;
      # CI tools are pinned to the flake's nixpkgs so workflows resolve exact
      # executables with `nix build .#<tool>` instead of trusting runner PATH.
      # cache-push uses attic/jq/xargs/gh; cve-scan uses curl/jq/tar, and its
      # PR gate uses node for ratchet-cli.mjs.
      # This avoids depending on a tool being on the runner PATH or a floating
      # `nixpkgs#` registry reference. The self-hosted runner PATH carries
      # coreutils + nix but not findutils, jq, gh, or node, so the bare
      # commands are `command not found` (cve-scan run 28598889924 died on
      # exactly that; the regression gate's ratchet step died the same way on
      # bare `node` in run 29196909666).
      inherit
        (pkgs)
        attic-client
        coreutils
        curl
        jq
        findutils
        gh
        gnutar
        nodejs
        ;
    }
    // repoFlakePackages
    // examplePackages
    // nonNixExampleImages
    // nonNixExampleDescriptions
    // crossPackages
    // healthChecks.lifecyclePackages;
  securityRootRegistry = let
    mkRoot = ix.securityRoots.mkRoot;
    owner = "indexable-inc/index";
    cachePolicy = {
      inherit owner;
      class = "cache-only";
      environment = "none";
      exposure = "none";
      criticality = "low";
      slaHours = 168;
    };
    baseImagePolicy = {
      inherit owner;
      class = "base-image";
      environment = "development";
      exposure = "internal";
      criticality = "medium";
      slaHours = 72;
    };
    # Business exposure is never inferred from package metadata. Add a complete
    # policy here only when a package is known to be deployed or distributed;
    # every unspecified non-image output remains cache hygiene, not exposure.
    securityRootPolicies = {};
    packageEntries =
      lib.mapAttrs (
        name: package: let
          isImage = package ? passthru.toplevel;
          path = package.passthru.toplevel or (lib.getOutput package.outputName package);
          policy =
            if isImage
            then baseImagePolicy
            else securityRootPolicies.${name} or cachePolicy;
        in {
          inherit path;
          root = mkRoot (
            {
              attr = "packages.${system}.${name}";
              inherit name;
            }
            // policy
          );
        }
      )
      packageSet;
    exampleEntries =
      lib.concatMapAttrs (
        fleetName: fleet:
          lib.mapAttrs' (
            node: path: let
              name = "example-${fleetName}-${node}";
            in
              lib.nameValuePair name {
                inherit path;
                root = mkRoot {
                  attr = "exampleFleets.${system}.${fleetName}.systemPackages.${node}";
                  inherit name owner;
                  class = "deployed-service";
                  environment = "development";
                  exposure = "internal";
                  criticality = "medium";
                  slaHours = 72;
                };
              }
          )
          fleet.systemPackages
      )
      exampleFleets;
    entries =
      if pkgs.stdenv.hostPlatform.isDarwin
      then packageEntries
      else packageEntries // exampleEntries;
  in {
    securityRoots = lib.mapAttrs (_: entry: entry.root) entries;
    securityRootPaths = lib.mapAttrs (_: entry: entry.path) entries;
  };
in {
  packages = packageSet;

  # Non-schema output consumed by update.yml via `nix eval --json`; see the
  # binding above for what it maps.
  inherit updatablePackages;

  # CI-only push roots for cache-push.yml. Two adjustments to `packages` keep the
  # cache useful to `ix up` while cutting the monolithic `*-oci.tar` archives that
  # dominate the run -- each is one uncompressed blob that never dedups, cold
  # every run since check.yml only eval-validates packages:
  #
  #   1. Every NixOS image is replaced by its `toplevel` closure -- the artifact
  #      `ix up` substitutes (consumers reconstruct the archive on demand via
  #      streamLayeredImage). Non-image packages, and non-NixOS OCI images (which
  #      expose no `toplevel`), pass through unchanged. See lib/image/oci-layer.nix.
  #   2. The `health-check-*` packages (and the `health-checks{,-zellij}` runners)
  #      pin every fleet node's `toplevel` closure as a build dep
  #      (lib/image/health-checks.nix). Drop the wrapper scripts and add the
  #      fleet node `toplevel` closures directly, so the closures those checks
  #      drag in stay cached without pushing the per-fleet script derivations.
  #   3. The cross lane's eval-time IFD outputs (`crossIfdRoots`): the rendered
  #      `cargo-units.nix`, its `cargo-unit-graph.json`, and the vendor dir a Mac
  #      forces at eval when it substitutes a Darwin cross output. These are
  #      build-time deps of the cross packages, so they are absent from those
  #      packages' runtime closures; adding them as roots is the fix for #1687.
  #      `crossPackageIfdRoots` extends this to cross packages that ride a second
  #      `buildWorkspace` (codex's codex-rs), whose own unit graph `crossIfdRoots`
  #      -- keyed off the shared `crossWorkspace` -- does not see.
  #   4. On Darwin hosts, the native lane's eval-time IFD outputs
  #      (`nativeIfdRoots`): the same three unit-graph artifacts as (3) but for
  #      the host's own target, which a Darwin consumer forces at eval when it
  #      evaluates any native wrapper (codex, claude-code) against the workspace
  #      unit graph. Runtime closures never carry them, so without explicit
  #      roots every Darwin consumer re-vendors and re-renders the graph at
  #      eval -- the same trap as (3), for the darwin cache lane (#1890).
  cachePushRoots = let
    # Per-node `health-check-*` lifecycle packages and the two
    # `health-checks{,-zellij}` runners all share the `health-check` prefix.
    isHealthCheck = lib.hasPrefix "health-check";
    imagesAsClosures = lib.mapAttrs (_: p: p.passthru.toplevel or p) (
      lib.filterAttrs (name: _: !isHealthCheck name) packageSet
    );
    # `fleet.systemPackages` keys each node's toplevel as `<node>-system`; the
    # fleet-name prefix keeps nodes sharing a name across fleets distinct.
    exampleNodeToplevels =
      lib.concatMapAttrs (
        fleetName: fleet:
          lib.mapAttrs' (
            node: toplevel: lib.nameValuePair "${fleetName}-${node}" toplevel
          )
          fleet.systemPackages
      )
      exampleFleets;
    # Native analog of `crossIfdRoots` (adjustment 4). `crossWorkspace` with no
    # target override IS the host workspace, so these are exactly the drvs a
    # Darwin consumer's eval of the native wrappers imports.
    nativeIfdRoots = lib.optionalAttrs pkgs.stdenv.hostPlatform.isDarwin {
      native-ifd-units-nix = crossWorkspace.units.unitsNix;
      native-ifd-unit-graph = crossWorkspace.units.unitGraphJson;
      native-ifd-vendor-dir = crossWorkspace.units.vendorDir;
    };
  in
    # Fleet node toplevels are NixOS closures: on Darwin they can only
    # eval-error (every `<fleet>-<node>` row in the first darwin lane run was
    # an eval failure, run 28762717645), so they stay a linux-lane concern.
    # Alias-shadowed natives (dag-runner, nix-web-monitor) need no exclusion
    # here: the flake grafts `linuxDarwinAliases` over this set, so the darwin
    # lane sees the cross drvs and its system filter drops them.
    if pkgs.stdenv.hostPlatform.isDarwin
    then imagesAsClosures // nativeIfdRoots
    else imagesAsClosures // exampleNodeToplevels // crossIfdRoots // crossPackageIfdRoots;

  # The policy manifest is safe to `nix eval --json`: derivations live in the
  # separate securityRootPaths output and must be realized before their terminal
  # store paths are trusted.
  inherit (securityRootRegistry) securityRoots securityRootPaths;

  inherit darwinPackageAliases;

  # Flat keying: one derivation per `checks.<system>.<name>`, as the flake schema
  # and `nix flake check` require. The `.#check` gate and blast-radius consume
  # the sharded `ciChecks` instead, so this output is not what CI enumerates.
  # `forkChecks` is merged on EVERY system (not just x86_64-linux like the
  # rest of `catalogFor`): the patched sources are cheap, platform-relevant
  # derivations, so `nix build .#checks.aarch64-darwin.patched-src-clippy`
  # validates the series against a local Darwin build right after a flake update.
  checks = catalogFor rustPackageTestSets.flat // forkChecks;
  # Closure build gates, keyed `<fork>.<patch>` (see the binding above). A
  # non-schema output like `ciChecks`, exposed per system so a darwin host can
  # gate-build natively before an upstream PR.
  inherit forkClosureGates;
  # Sharded keying for the memory-bounded CI evaluator (nix-fast-build /
  # nix-eval-jobs / blast-radius): each package's per-#[test] checks sit under one
  # `recurseForDerivations` group, so the evaluator lists cheap per-package names
  # at the root and forces each crate's manifest IFD in its own worker job
  # (ENG-2201). Not a `checks.<system>.<name>` output, because a non-derivation
  # there fails the flake schema. The patched-src checks are plain derivations,
  # so they key identically in both views.
  ciChecks = catalogFor rustPackageTestSets.sharded // forkChecks;

  formatter = pkgs.alejandra;

  # `nix run .#bench` runs the repo's self-demo perf job (timing + RSS + custom
  # metrics, gated on regressions). The flake's package-with-mainProgram
  # convention already gives `nix run .#indexbench` for the bare CLI; this `apps`
  # entry is the named perf-job entry point the framework documents.
  apps = {
    bench = {
      type = "app";
      program = lib.getExe indexbenchSelfDemo.app;
      meta.description = "Run the indexbench self-demo perf suite";
    };
  };

  # `nix develop .#bench` drops into a shell with the bench + profiling tools.
  # tango is already a workspace dependency (built per-crate by cargo-unit); the
  # shell adds the out-of-process profilers a bench author reaches for.
  devShells = {
    default = pkgs.mkShellNoCC {
      packages = [
        repoPackages.astlog
        pkgs.alejandra
      ];
    };

    bench = pkgs.mkShellNoCC {
      packages = [
        indexbench
        pkgs.hyperfine
        pkgs.valgrind
        pkgs.samply
        pkgs.jemalloc
      ];
    };
  };
}
