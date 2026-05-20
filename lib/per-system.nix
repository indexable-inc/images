# Per-system flake outputs (packages / apps / checks / formatter).
#
# Kept out of flake.nix so the flake top-level can read as a manifest of
# inputs and output categories. Composition logic for apps and lint plumbing
# lives here.
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

  mkApp = program: description: {
    type = "app";
    program = lib.getExe program;
    meta = { inherit description; };
  };

  lint = ix.writeNushellApplication pkgs {
    name = "lint";
    runtimeInputs = [
      pkgs.ast-grep
      pkgs.deadnix
      pkgs.fd
      pkgs.nixfmt
      pkgs.statix
    ];
    text = ''
      def main [] {
        let nix_files = (fd --extension nix | lines)

        print "nixfmt"
        nixfmt --check ...$nix_files

        print "statix"
        statix check .

        print "deadnix"
        deadnix --fail --no-lambda-pattern-names .

        print "ast-grep"
        ast-grep scan --error .
      }
    '';
  };

  updateMods = ix.writePythonApplication pkgs {
    name = "update-mods";
    src = paths.tools.updateMods;
  };

  updateIxCli = ix.writePythonApplication pkgs {
    name = "update-ix-cli";
    src = paths.tools.updateIxCli;
    runtimeInputs = [ pkgs.nix ];
  };

  ixShellSyncIgnored = ix.writePythonApplication pkgs {
    name = "ix-shell-sync-ignored";
    src = paths.tools.ixShellSyncIgnored;
    runtimeInputs = [
      pkgs.git
      pkgs.gnutar
    ];
  };

  mcSource = ix.writeNushellApplication pkgs {
    name = "mc-source";
    text = builtins.readFile paths.tools.mcSource;
    runtimeInputs = [
      (pkgs.callPackage paths.packages.vineflower { })
    ];
  };

  benchFilesystem = import paths.bench.filesystem { inherit ix pkgs; };

  repoPackages = ix.packageSetFor pkgs;

  rustPackageTests =
    let
      rustPackages = lib.getAttrs [
        "minecraft-nbt"
        "minecraft-sync-managed"
        "nix-cargo-unit"
        "oci-image-builder"
      ] repoPackages;
    in
    lib.concatMapAttrs (
      packageName: package:
      lib.mapAttrs' (testName: test: lib.nameValuePair "rust-${packageName}-${testName}" test) (
        package.passthru.tests or { }
      )
    ) rustPackages;

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

  # Surface every example's `ix fleet <sub>` wrapper as a flake app.
  # Each example contributes `apps.<system>.<example>-{up,health,...}`,
  # which lets `nix run .#nginx-lifecycle-up` invoke the existing fleet
  # plumbing without going through `nix build` + `result/bin/...`.
  exampleApps =
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
          value = mkApp fleet.${sub} "Run `ix fleet ${sub}` against the ${name} example fleet";
        }) fleetSubs
      )
    ) exampleFleets;

  healthChecks = import ./health-checks.nix {
    inherit lib pkgs;
    inherit (ix) writeNushellApplication;
    dagRunner = repoPackages.dag-runner;
  } { exampleFleets = healthCheckExampleFleets; };
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

      health-checks = healthChecks;
      inherit (repoPackages)
        dag-runner
        drgn
        ix-fleet
        mc-probe
        minecraft-nbt
        minecraft-sync-managed
        llm-clippy
        nix-cargo-unit
        oci-image-builder
        python-mcp-server
        ;
      minestom-hello-server-jar = repoPackages.minestom.helloServerJar;
    }
    // lib.optionalAttrs (repoPackages ? ix) {
      inherit (repoPackages) ix;
    }
    // lib.optionalAttrs (system == ix.system) {
      inherit (repoPackages) tonbo-artifacts;
    };

  apps = {
    lint = mkApp lint "Run all Nix formatting and lint checks";
    bench-filesystem = mkApp benchFilesystem "Benchmark file-system behavior from inside an ix VM";
    update-mods = mkApp updateMods "Regenerate Minecraft mod catalogs";
    update-ix-cli = mkApp updateIxCli "Re-prefetch the ix.dev CLI binaries and bump packages/ix/default.nix hashes";
    ix-fleet = mkApp repoPackages.ix-fleet "Render ix fleet plans and commands";
    ix-shell-sync-ignored = mkApp ixShellSyncIgnored "Copy git-ignored files into an ix shell workspace";
    mc-source = mkApp mcSource "Decompile a Minecraft server jar with Mojang mappings via Vineflower";
    nix-cargo-unit = mkApp repoPackages.nix-cargo-unit "Render Cargo unit graphs as Nix derivations";
    python-mcp-server = mkApp repoPackages.python-mcp-server "Run a Python MCP server";
    health-checks = mkApp healthChecks "Boot every example fleet in parallel, run its health checks, and tear the VMs down";
  }
  // exampleApps;

  checks =
    lib.optionalAttrs (system == ix.system) {
      inherit (tests) eval;
      cargo-unit-real-workspaces = tests.cargoUnitRealWorkspaces;
      lint = pkgs.runCommand "ix-images-lint" { nativeBuildInputs = [ pkgs.coreutils ]; } ''
        cp -R ${lintSource} source
        chmod -R u+w source
        cd source
        ${lib.getExe lint}
        mkdir -p "$out"
      '';
    }
    // lib.optionalAttrs (system == ix.system) rustPackageTests;

  formatter = pkgs.nixfmt;
}
