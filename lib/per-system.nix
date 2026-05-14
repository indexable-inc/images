# Per-system flake outputs (packages / apps / checks / formatter).
#
# Kept out of flake.nix so the flake top-level can read as a manifest of
# inputs and output categories. All composition logic for apps, demo
# wrappers, and lint plumbing lives here.
{
  system,
  ix,
  nixpkgs,
  paths,
}:
let
  inherit (nixpkgs) lib;
  pkgs = nixpkgs.legacyPackages.${system};
  fs = lib.fileset;

  mkApp = program: description: {
    type = "app";
    program = lib.getExe program;
    meta = { inherit description; };
  };

  pythonWithPydantic = pkgs.python3.withPackages (ps: [ ps.pydantic ]);

  mkPythonWrapper =
    {
      name,
      script,
      python ? pkgs.python3,
    }:
    ix.writeNushellApplication pkgs {
      inherit name;
      runtimeInputs = [ python ];
      text = ''
        def main [...args] {
          exec python3 ${script} ...$args
        }
      '';
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

  updateMods = mkPythonWrapper {
    name = "update-mods";
    script = paths.tools.updateMods;
  };

  ixFleet = mkPythonWrapper {
    name = "ix-fleet";
    script = paths.tools.ixFleet;
    python = pythonWithPydantic;
  };

  benchFilesystem = import paths.bench.filesystem { inherit ix pkgs; };

  claudeCodeDemo = import paths.examples.claudeCodeDemo {
    ix = {
      lib = ix;
    };
    hostSystem = system;
  };
  claudeCodeDemoImages = lib.mapAttrs' (
    name: package: lib.nameValuePair "claude-code-demo-${name}-image" package
  ) claudeCodeDemo.packages;
  mkDemoVmUp =
    vm:
    ix.writeNushellApplication pkgs {
      name = "claude-code-demo-${vm}-up";
      runtimeInputs = [ claudeCodeDemo.up ];
      text = ''
        def --wrapped main [...args] {
          exec ix-fleet-up --on ${vm} ...$args
        }
      '';
    };
  claudeCodeDemoLinuxUp = mkDemoVmUp "linux";
  claudeCodeDemoMinecraftUp = mkDemoVmUp "minecraft";

  repoPackages = ix.packageSetFor pkgs;

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
    // claudeCodeDemo.systemPackages
    // claudeCodeDemoImages
    // {
      claude-code-demo-command = claudeCodeDemo.command;
      claude-code-demo-diff = claudeCodeDemo.diff;
      claude-code-demo-plan = claudeCodeDemo.planCommand;
      claude-code-demo-replace = claudeCodeDemo.replace;
      claude-code-demo-switch = claudeCodeDemo.switch;
      claude-code-demo-up = claudeCodeDemo.up;
      claude-code-demo-linux-up = claudeCodeDemoLinuxUp;
      claude-code-demo-minecraft-up = claudeCodeDemoMinecraftUp;
      minestom-hello-server-jar = repoPackages.minestom.helloServerJar;
    }
    // lib.optionalAttrs (system == ix.system) {
      inherit (repoPackages) tonbo-artifacts;
    };

  apps = {
    lint = mkApp lint "Run all Nix formatting and lint checks";
    bench-filesystem = mkApp benchFilesystem "Benchmark file-system behavior from inside an ix VM";
    update-mods = mkApp updateMods "Regenerate Minecraft mod catalogs";
    ix-fleet = mkApp ixFleet "Render ix fleet plans and commands";
    claude-code-demo-diff = mkApp claudeCodeDemo.diff "Diff the Claude Code demo fleet against live VMs";
    claude-code-demo-plan = mkApp claudeCodeDemo.planCommand "Render the Claude Code demo fleet plan";
    claude-code-demo-replace = mkApp claudeCodeDemo.replace "Build replacement images for the Claude Code demo fleet";
    claude-code-demo-up = mkApp claudeCodeDemo.up "Build and upload demo OCI images, then create or start VMs from them";
    claude-code-demo-linux-up = mkApp claudeCodeDemoLinuxUp "Build and upload the Claude Code demo Linux image, then create or start only the Linux VM";
    claude-code-demo-minecraft-up = mkApp claudeCodeDemoMinecraftUp "Build and upload the Claude Code demo Minecraft image, then create or start only the Minecraft VM";
    claude-code-demo-switch = mkApp claudeCodeDemo.switch "Switch the Claude Code demo fleet";
  };

  checks = lib.optionalAttrs (system == ix.system) {
    inherit (tests) eval;
    lint = pkgs.runCommand "ix-images-lint" { nativeBuildInputs = [ pkgs.coreutils ]; } ''
      cp -R ${lintSource} source
      chmod -R u+w source
      cd source
      ${lib.getExe lint}
      mkdir -p "$out"
    '';
  };

  formatter = pkgs.nixfmt;
}
