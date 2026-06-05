{
  ix,
  lib,
  pkgs ? ix.pkgs,
}:

let
  dagRunner = pkgs.callPackage ../dag-runner { inherit ix; };
  # The ix Python SDK is a prebuilt wheel fetched from R2, not a uv/PyPI
  # dependency, so it is injected into the venv below rather than resolved by uv.
  ixSdk = pkgs.callPackage ../ix-sdk-python { };

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

  # Drives the full `up` workflow under --dry-run: it makes no API calls and
  # touches no network, so it runs in the sandbox, yet it exercises the rewritten
  # control flow and (because the module imports ix_sdk at load) proves the
  # prebuilt SDK wheel is importable from the built venv. The CLI-stubbing test
  # this replaces no longer fits now that fleet ops go through the SDK, not a
  # subprocess; live SDK behavior is covered by the example health-checks.
  dryRunUp =
    pkgs.runCommand "ix-fleet-dry-run-up"
      {
        nativeBuildInputs = [ package ];
        strictDeps = true;
      }
      ''
        ix-fleet --plan ${dryRunPlan} up --skip-push --skip-health --dry-run
        mkdir -p "$out"
      '';

  package = unwrapped.overrideAttrs (old: {
    postInstall = ''
      ${old.postInstall or ""}
      # Drop the prebuilt ix_sdk wheel into the venv site-packages so `import
      # ix_sdk` resolves both at runtime and for the ty install check, without a
      # PYTHONPATH shim. The cdylib comes from R2 (packages/ix-sdk-python).
      cp -r ${ixSdk}/${pkgs.python3.sitePackages}/. "$out/venv/${pkgs.python3.sitePackages}/"

      wrapProgram "$out/bin/ix-fleet" \
        --set IX_FLEET_DAG_RUNNER ${lib.escapeShellArg (lib.getExe dagRunner)}
    '';

    passthru = (old.passthru or { }) // {
      tests = (old.passthru.tests or { }) // {
        inherit dryRunUp;
      };
    };
  });
in
package
