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
}:
let
  inherit (nixpkgs) lib;
  pkgs = import nixpkgs {
    inherit system;
    overlays = [
      rust-overlay.overlays.default
      ix.overlay
    ];
  };
  fs = lib.fileset;
  packageRegistry = import (paths.packagesRoot + "/registry.nix") {
    inherit lib;
    root = paths.packagesRoot;
  };

  # Each lint stage is one subcommand on a single binary so the spec keys
  # off `lib.getExe lintStage` without registering four sibling packages.
  # The Nu wrapper checks syntax at build time, so a typo in a stage shows
  # up in the `lint` derivation build, not at `nix run` time.
  lintStage = ix.writeNushellApplication pkgs {
    name = "lint-stage";
    meta.description = "One lint stage (nixfmt | statix | deadnix | ast-grep | ast-grep-test); driven by `lint`";
    runtimeInputs = [
      pkgs.ast-grep
      pkgs.deadnix
      pkgs.fd
      pkgs.nixfmt
      pkgs.statix
    ];
    text = ''
      def "main nixfmt" [] {
        let nix_files = (fd --extension nix | lines)
        nixfmt --check ...$nix_files
      }
      def "main statix" [] { statix check . }
      def "main deadnix" [] { deadnix --fail --no-lambda-pattern-names . }
      def "main ast-grep" [] { ast-grep scan --error . }
      # Rule self-test: every fixture under nix-rules-tests must flag its
      # invalid cases and ignore its valid ones. Catches rules whose pattern
      # silently stops matching (e.g. a bare `attr = val` that parses as an
      # expression, not a binding). --skip-snapshot-tests keeps it to match
      # presence/absence without baseline snapshot files.
      def "main ast-grep-test" [] { ast-grep test --skip-snapshot-tests }
      def main [] {
        error make { msg: "specify a stage: nixfmt | statix | deadnix | ast-grep | ast-grep-test" }
      }
    '';
  };

  lintSpec = (pkgs.formats.json { }).generate "lint-dag.json" {
    nodes = {
      nixfmt.command = [
        (lib.getExe lintStage)
        "nixfmt"
      ];
      statix.command = [
        (lib.getExe lintStage)
        "statix"
      ];
      deadnix.command = [
        (lib.getExe lintStage)
        "deadnix"
      ];
      "ast-grep".command = [
        (lib.getExe lintStage)
        "ast-grep"
      ];
      "ast-grep-test".command = [
        (lib.getExe lintStage)
        "ast-grep-test"
      ];
    };
  };

  lint = ix.writeNushellApplication pkgs {
    name = "lint";
    meta.description = "Run all Nix formatting and lint checks in parallel via dag-runner";
    runtimeInputs = [ repoPackages.dag-runner ];
    text = ''
      def --wrapped main [...args] {
        exec dag-runner ...$args ${lintSpec}
      }
    '';
  };

  updateMods = ix.writePythonApplication pkgs {
    name = "update-mods";
    src = paths.tools.updateMods;
    meta.description = "Regenerate Minecraft mod catalogs";
  };

  updateLoaders = ix.writePythonApplication pkgs {
    name = "update-loaders";
    src = paths.tools.updateLoaders;
    meta.description = "Refresh Minecraft loader (Paper / Velocity / Fabric) catalogs from upstream";
  };

  updateIxCli = ix.writePythonApplication pkgs {
    name = "update-ix-cli";
    src = paths.tools.updateIxCli;
    runtimeInputs = [ pkgs.nix ];
    meta.description = "Re-prefetch the ix.dev CLI binaries and bump packages/ix/default.nix hashes";
  };

  ixShellSyncIgnored = ix.writePythonApplication pkgs {
    name = "ix-shell-sync-ignored";
    src = paths.tools.ixShellSyncIgnored;
    runtimeInputs = [
      pkgs.git
      pkgs.gnutar
    ];
    meta.description = "Copy git-ignored files into an ix shell workspace";
  };

  agentsMd = repoPackages.agents-md;

  mcSource = ix.writeNushellApplication pkgs {
    name = "mc-source";
    text = builtins.readFile paths.tools.mcSource;
    runtimeInputs = [
      (pkgs.callPackage packageRegistry.byId.vineflower.path { })
    ];
    meta.description = "Decompile a Minecraft server jar with Mojang mappings via Vineflower";
  };

  benchFilesystem = import paths.bench.filesystem { inherit ix pkgs; };

  siteSrc = fs.toSource {
    root = paths.site;
    fileset = fs.intersection (fs.gitTracked paths.site) (
      fs.unions [
        (paths.site + "/package.json")
        (paths.site + "/package-lock.json")
        (paths.site + "/mdsvex.config.js")
        (paths.site + "/svelte.config.js")
        (paths.site + "/vite.config.ts")
        (paths.site + "/vitest.config.ts")
        (paths.site + "/tsconfig.json")
        (paths.site + "/eslint.config.js")
        (paths.site + "/src")
        (paths.site + "/static")
      ]
    );
  };

  siteBuild = ix.buildSvelteSite pkgs {
    pname = "ix-site";
    version = "0.1.0";
    src = siteSrc;
    distDir = "build";
    serve = {
      name = "ix-site";
      routePrefix = "/index";
    };
    devServer = {
      name = "ix-site-dev";
      checkoutSubdir = "site";
    };
  };

  # The local preview serves the same `/index` build that Pages deploys.
  site = siteBuild.overrideAttrs (old: {
    passthru = (old.passthru or { }) // {
      preview = siteBuild.passthru.serve;
      static = siteBuild.passthru.staticSite;
    };
  });

  siteTests = ix.buildNpmVitest pkgs {
    pname = "ix-site";
    version = "0.1.0";
    src = siteSrc;
    preTest = ''
      node node_modules/@sveltejs/kit/src/cli.js sync
    '';
  };

  repoPackages = ix.packageSetFor pkgs;
  repoFlakePackages = lib.listToAttrs (
    map (
      entry:
      lib.nameValuePair entry.flake.attrName (
        lib.attrByPath entry.packageSet.attrPath
          (throw "packages/${entry.relativePath}/package.nix: flake output `${entry.flake.attrName}` needs packageSet.attrPath")
          repoPackages
      )
    ) (packageRegistry.flakeEntriesFor system)
  );

  rustPackageTests =
    let
      repoRustPackageTests = lib.mergeAttrsList (
        map (
          entry:
          let
            package =
              lib.attrByPath entry.packageSet.attrPath
                (throw "packages/${entry.relativePath}/package.nix: passthruTests needs packageSet.attrPath")
                repoPackages;
          in
          lib.mapAttrs' (testName: test: lib.nameValuePair "${entry.passthruTests.prefix}-${testName}" test) (
            package.passthru.tests or { }
          )
        ) (packageRegistry.passthruTestEntriesFor system)
      );
      moduleRustPackages = {
        resource-monitor-stats-writer =
          let
            cargoUnit = ix.cargoUnitFor pkgs;
            rustWorkspace = ix.rustWorkspaceFor pkgs;
          in
          cargoUnit.selectBinaryWithTests rustWorkspace.units {
            binary = "resource-monitor-stats-writer";
          };
      };
      moduleRustPackageTests = lib.concatMapAttrs (
        packageName: package:
        lib.mapAttrs' (testName: test: lib.nameValuePair "rust-${packageName}-${testName}" test) (
          package.passthru.tests or { }
        )
      ) moduleRustPackages;
    in
    repoRustPackageTests // moduleRustPackageTests;

  lintSource = fs.toSource {
    inherit (paths) root;
    fileset = fs.gitTracked paths.root;
  };

  tests = import paths.tests { inherit nixpkgs ix; };

  exampleFleets = ix.exampleFleetsFor { hostSystem = system; };

  # Separate aggregation with "health-check-" prepended to every node name,
  # so the lifecycle scripts that force-delete VMs by name can never clobber
  # an unrelated production VM that happens to share the example's node name
  # (`nginx`, `factions`, ...).
  healthCheckExampleFleets = ix.exampleFleetsFor {
    hostSystem = system;
    nodePrefix = "health-check-";
  };

  # Surface every example's `ix fleet <sub>` wrapper as a flake package.
  # Each example contributes `packages.<system>.<example>-{up,health,...}`,
  # which lets `nix run .#nginx-lifecycle-up` invoke the existing fleet
  # plumbing through the wrapper's `meta.mainProgram`, and
  # `nix build .#nginx-lifecycle-up` produce the wrapper script on disk.
  examplePackages =
    let
      fleetSubs = [
        "up"
        "health"
        "replace"
        "switch"
        "diff"
      ];
    in
    lib.concatMapAttrs (
      name: fleet:
      lib.listToAttrs (
        map (sub: {
          name = "${name}-${sub}";
          value = fleet.${sub}.overrideAttrs (old: {
            meta = (old.meta or { }) // {
              description = "Run `ix fleet ${sub}` against the ${name} example fleet";
            };
          });
        }) fleetSubs
      )
    ) exampleFleets;

  healthChecks =
    import ./health-checks.nix
      {
        inherit lib pkgs;
        inherit (ix) writeNushellApplication;
        dagRunner = repoPackages.dag-runner;
      }
      {
        exampleFleets = healthCheckExampleFleets;
        exampleNames = lib.attrNames exampleFleets;
      };
in
{
  packages =
    (ix.discoverImages {
      root = paths.images;
      inherit (tests) imageTests;
    })
    // {
      base =
        let
          package = ix.mkImage {
            modules = [
              {
                ix.image = {
                  name = "ix/base";
                  tag = "latest";
                };
              }
            ];
          };
        in
        package
        // {
          passthru = (package.passthru or { }) // {
            tests = (package.passthru.tests or { }) // {
              eval = tests.imageTests.base;
            };
          };
        };

      health-checks = healthChecks.dag;
      health-checks-zellij = healthChecks.zellij;
      inherit lint site;
      site-dev = site.passthru.devServer;
      bench-filesystem = benchFilesystem;
      update-mods = updateMods;
      update-loaders = updateLoaders;
      update-ix-cli = updateIxCli;
      ix-shell-sync-ignored = ixShellSyncIgnored;
      mc-source = mcSource;
    }
    // repoFlakePackages
    // examplePackages
    // healthChecks.lifecyclePackages;

  checks = lib.optionalAttrs (system == ix.system) (
    {
      inherit (tests) eval;
      agents-md = pkgs.runCommand "agents-md-check" { nativeBuildInputs = [ agentsMd ]; } ''
        agents-md --check ${paths.root}
        mkdir -p "$out"
      '';
      cargo-unit-real-workspaces = tests.cargoUnitRealWorkspaces;
      # Offline schema gate for the loader manifests. `deepSeq` forces
      # every Paper / Velocity / Fabric per-version lock through
      # `readLoaderManifest` in `lib/artifacts.nix`, so malformed JSON or a
      # missing key fires here before any image starts evaluating. The
      # forced surface is the parsed-and-validated manifest data, not the
      # wrapped `fetchurl` derivations, to keep this check pure eval.
      loader-manifests =
        let
          forced = builtins.deepSeq ix.artifacts.minecraft.loaderManifests "ok";
        in
        pkgs.runCommand "loader-manifests-check" { } ''
          printf '%s\n' '${forced}' > "$out"
        '';
      run-records-session = repoPackages.run.passthru.tests.recordsSession;
      lint = pkgs.runCommand "ix-images-lint" { nativeBuildInputs = [ pkgs.coreutils ]; } ''
        cp -R ${lintSource} source
        chmod -R u+w source
        cd source
        ${lib.getExe lint}
        mkdir -p "$out"
      '';
      site-test = siteTests.all;
    }
    // lib.mapAttrs' (caseId: drv: lib.nameValuePair "site-test-${caseId}" drv) siteTests.cases
    // rustPackageTests
  );

  formatter = pkgs.nixfmt;
}
