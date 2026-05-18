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
    overlays = [ rust-overlay.overlays.default ];
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

  ixShellSyncIgnored = ix.writePythonApplication pkgs {
    name = "ix-shell-sync-ignored";
    src = paths.tools.ixShellSyncIgnored;
    runtimeInputs = [
      pkgs.git
      pkgs.gnutar
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

      inherit (repoPackages)
        hyperion
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
    ix-fleet = mkApp repoPackages.ix-fleet "Render ix fleet plans and commands";
    ix-shell-sync-ignored = mkApp ixShellSyncIgnored "Copy git-ignored files into an ix shell workspace";
    nix-cargo-unit = mkApp repoPackages.nix-cargo-unit "Render Cargo unit graphs as Nix derivations";
    python-mcp-server = mkApp repoPackages.python-mcp-server "Run a Python MCP server";
  };

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
