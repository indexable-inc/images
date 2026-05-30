{
  ix,
  lib,
  pkgs ? ix.pkgs,
}:

let
  dagRunner = pkgs.callPackage ../dag-runner { inherit ix; };
  unwrapped = ix.buildUvApplication pkgs {
    pname = "ix-fleet";
    version = "0.1.0";
    srcRoot = ./.;
  };

  jsonFormat = pkgs.formats.json { };
  dryRunPlan = jsonFormat.generate "ix-fleet-dry-run-plan.json" {
    order = [ "api" ];
    nodes.api = {
      name = "api";
      baseName = "api";
      system = "/nix/store/api-system";
      switch = {
        target = "/nix/store/api-system";
        sourceInstallable = ".#api";
      };
      bootstrapImage = "registry.ix.dev/ix/base:latest";
      replacementImage = {
        imageName = "api";
        imageTag = "latest";
        destination = "registry.ix.dev/example/api:latest";
        source = "/nix/store/api-image.tar";
        sourceDrv = "/nix/store/api-image.drv";
      };
      region = "us-west-1";
      ipv4 = false;
      snapshot = false;
    };
  };

  fakeIx = ix.writeNushellApplication pkgs {
    name = "ix";
    text = ''
      def --wrapped main [command: string, ...args] {
        match $command {
          "ls" => { print "[]" }
          "new" => { }
          _ => {
            print -e $"unexpected ix command: ($command) (($args | str join ' '))"
            exit 1
          }
        }
      }
    '';
  };

  upFindsDagRunner =
    pkgs.runCommand "ix-fleet-up-finds-dag-runner"
      {
        nativeBuildInputs = [
          fakeIx
          package
        ];
        strictDeps = true;
      }
      ''
        ix-fleet --plan ${dryRunPlan} up --skip-push --skip-health
        mkdir -p "$out"
      '';

  package = unwrapped.overrideAttrs (old: {
    postInstall = ''
      ${old.postInstall or ""}
      wrapProgram "$out/bin/ix-fleet" \
        --set IX_FLEET_DAG_RUNNER ${lib.escapeShellArg (lib.getExe dagRunner)}
    '';

    passthru = (old.passthru or { }) // {
      tests = (old.passthru.tests or { }) // {
        inherit upFindsDagRunner;
      };
    };
  });
in
package
