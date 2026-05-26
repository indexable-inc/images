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

  # Each lint stage is one subcommand on a single binary so the spec keys
  # off `lib.getExe lintStage` without registering four sibling packages.
  # The Nu wrapper checks syntax at build time, so a typo in a stage shows
  # up in the `lint` derivation build, not at `nix run` time.
  lintStage = ix.writeNushellApplication pkgs {
    name = "lint-stage";
    meta.description = "One lint stage (nixfmt | statix | deadnix | ast-grep); driven by `lint`";
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
      def main [] {
        error make { msg: "specify a stage: nixfmt | statix | deadnix | ast-grep" }
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

  agentsMdFile = pkgs.writeText "AGENTS.md" (ix.agentsMd.render { });

  agentsMd = ix.writeNushellApplication pkgs {
    name = "agents-md";
    meta.description = "Generate this repository's AGENTS.md from reusable fragments";
    text = ''
      def main [
        --write: path
        --check: path
      ] {
        let generated = (open --raw ${agentsMdFile})

        if $check != null {
          let current = (open --raw $check)
          if $current != $generated {
            print --stderr $"($check) differs from generated AGENTS.md"
            exit 1
          }
          return
        }

        if $write != null {
          $generated | save --force $write
          return
        }

        print --no-newline $generated
      }
    '';
  };

  # Bake the repo's lint program into the loop runner so
  # `nix run .#loop` matches the historical Python wrapper's UX. The
  # underlying binary still accepts `--lint-program` as an override.
  loop =
    pkgs.runCommand "loop"
      {
        nativeBuildInputs = [ pkgs.makeWrapper ];
        meta = {
          mainProgram = "loop";
          description = "Run an agent CLI in a checked commit-and-push loop with a live web UI";
        };
      }
      ''
        mkdir -p $out/bin
        makeWrapper ${lib.getExe repoPackages.loop} $out/bin/loop \
          --add-flags --lint-program \
          --add-flags ${lib.escapeShellArg (lib.getExe lint)} \
          --prefix PATH : ${
            lib.makeBinPath [
              pkgs.git
              pkgs.mgrep
            ]
          }
      '';

  mcSource = ix.writeNushellApplication pkgs {
    name = "mc-source";
    text = builtins.readFile paths.tools.mcSource;
    runtimeInputs = [
      (pkgs.callPackage paths.packages.vineflower { })
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

  siteBuild = ix.buildNpmSite pkgs {
    pname = "ix-site";
    version = "0.1.0";
    src = siteSrc;
    distDir = "build";
  };

  # Local preview build: same source, but with the SvelteKit base path
  # cleared so miniserve can serve it from the URL root. The deployed
  # artifact (`siteBuild`) keeps the `/index` prefix for GitHub Pages.
  sitePreviewBuild = ix.buildNpmSite pkgs {
    pname = "ix-site-preview";
    version = "0.1.0";
    src = siteSrc;
    distDir = "build";
    preBuild = "export BASE_PATH=";
    installDir = "share/ix-site-preview";
  };

  sitePreviewServe =
    pkgs.runCommand "ix-site-serve"
      {
        nativeBuildInputs = [ pkgs.makeBinaryWrapper ];
      }
      ''
        mkdir -p $out/bin
        makeWrapper ${lib.getExe pkgs.miniserve} $out/bin/ix-site \
          --add-flags "--index index.html --interfaces 127.0.0.1 --port 8080 ${sitePreviewBuild}/share/ix-site-preview"
      '';

  site = pkgs.symlinkJoin {
    name = "ix-site-0.1.0";
    paths = [
      siteBuild
      sitePreviewServe
    ];
    meta.mainProgram = "ix-site";
  };

  siteTests = ix.buildNpmVitest pkgs {
    pname = "ix-site";
    version = "0.1.0";
    src = siteSrc;
    preTest = ''
      node node_modules/@sveltejs/kit/src/cli.js sync
    '';
  };

  repoPackages = ix.packageSetFor pkgs;

  rustPackageTests =
    let
      rustPackages = lib.getAttrs [
        "dag-runner"
        "ix-dev-diagnose"
        "loop"
        "mcp"
        "minecraft-nbt"
        "minecraft-sync-managed"
        "nix-cargo-unit"
        "oci-image-builder"
        "room"
      ] repoPackages;
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
    in
    lib.concatMapAttrs (
      packageName: package:
      lib.mapAttrs' (testName: test: lib.nameValuePair "rust-${packageName}-${testName}" test) (
        package.passthru.tests or { }
      )
    ) (rustPackages // moduleRustPackages);

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
        inherit (repoPackages) loop;
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
      health-checks-loro = healthChecks.loro;
      health-checks-zellij = healthChecks.zellij;
      inherit lint loop site;
      agents-md = agentsMd;
      bench-filesystem = benchFilesystem;
      update-mods = updateMods;
      update-ix-cli = updateIxCli;
      ix-shell-sync-ignored = ixShellSyncIgnored;
      mc-source = mcSource;
      inherit (repoPackages)
        dag-runner
        drgn
        ix-dev-diagnose
        ix-fleet
        mc-probe
        minecraft-nbt
        minecraft-sync-managed
        llm-clippy
        nix-cargo-unit
        oci-image-builder
        run
        mcp
        ;
      minestom-hello-server-jar = repoPackages.minestom.helloServerJar;
      inherit (repoPackages) room;
    }
    // examplePackages
    // healthChecks.lifecyclePackages
    // lib.optionalAttrs (repoPackages ? ix) {
      inherit (repoPackages) ix;
    }
    // lib.optionalAttrs (system == ix.system) {
      inherit (repoPackages) tonbo-artifacts;
    };

  checks =
    lib.optionalAttrs (system == ix.system) {
      inherit (tests) eval;
      agents-md = pkgs.runCommand "agents-md-check" { nativeBuildInputs = [ agentsMd ]; } ''
        agents-md --check ${paths.root}/AGENTS.md
        mkdir -p "$out"
      '';
      cargo-unit-real-workspaces = tests.cargoUnitRealWorkspaces;
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
    // lib.optionalAttrs (system == ix.system) (
      lib.mapAttrs' (caseId: drv: lib.nameValuePair "site-test-${caseId}" drv) siteTests.cases
    )
    // lib.optionalAttrs (system == ix.system) rustPackageTests;

  formatter = pkgs.nixfmt;
}
