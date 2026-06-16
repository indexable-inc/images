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
      # Exercise the declarative per-VM secret refs through the dry-run create
      # path (no live call, so the names need not exist in any store here).
      secrets = [ "GH_TOKEN" ];
      noDefaultSecrets = true;
    };
  };

  # Walks the `up` command's --dry-run control flow (no API calls, no network,
  # so it runs in the sandbox) and, because the module imports ix_sdk at load,
  # proves the prebuilt SDK wheel is importable from the built venv. Note this
  # only covers the dry-run branches: the live SDK calls and the dag-runner
  # fan-out are not exercised here (the SDK can't be stubbed by a fake CLI like
  # the old test did); that path is covered by the example health-checks.
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

  # Two remote-source nodes that share a build VM, so `switch --dry-run` exercises
  # the native multi-VM batch path: both must land in one `ix up .#web .#worker
  # --build-vm builder` command.
  dryRunSwitchPlan = jsonFormat.generate "ix-fleet-dry-run-switch-plan.json" {
    order = [
      "web"
      "worker"
    ];
    nodes = lib.genAttrs [ "web" "worker" ] (name: {
      inherit name;
      baseName = name;
      system = "/nix/store/${name}-system";
      switch = {
        target = "/nix/store/${name}-system.drv";
        buildOn = "remote";
        buildVm = "builder";
        sourceInstallable = ".#${name}";
      };
      bootstrapImage = "registry.ix.dev/ix/base:latest";
      replacementImage = {
        imageName = name;
        imageTag = "latest";
        destination = "registry.ix.dev/example/${name}:latest";
        source = "/nix/store/${name}-image.tar";
        sourceDrv = "/nix/store/${name}-image.drv";
      };
      region = "us-west-1";
      ipv4 = false;
      snapshot = false;
    });
  };

  # Walks the `switch` command's --dry-run control flow and asserts the two
  # batchable nodes collapse into one native multi-VM `ix up` invocation rather
  # than one per node. No API calls or network, so it runs in the sandbox.
  dryRunSwitch =
    pkgs.runCommand "ix-fleet-dry-run-switch"
      {
        nativeBuildInputs = [ package ];
        strictDeps = true;
      }
      ''
        ix-fleet --plan ${dryRunSwitchPlan} switch --skip-health --no-snapshot --dry-run | tee switch.log
        grep -qE '\+ ix up \.#web \.#worker --build-vm builder' switch.log \
          || { echo "expected a single batched 'ix up .#web .#worker --build-vm builder'" >&2; exit 1; }
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
        inherit dryRunUp dryRunSwitch;
      };
    };
  });
in
package
