{
  ix,
  lib,
  pkgs ? ix.pkgs,
}: let
  dagRunner = pkgs.callPackage (ix.paths.packagesRoot + "/dag-runner") {inherit ix;};
  # The ix Python SDK is a prebuilt wheel fetched from R2, not a uv/PyPI
  # dependency, so it is injected into the venv below rather than resolved by uv.
  ixSdk = pkgs.callPackage (ix.paths.packagesRoot + "/ix-sdk-python") {inherit ix;};

  unwrapped = ix.buildUvApplication pkgs {
    pname = "ix-fleet";
    version = "0.1.0";
    srcRoot = ./.;
    pyChecker = "zuban";
  };

  jsonFormat = pkgs.formats.json {};
  dryRunPlan = jsonFormat.generate "ix-fleet-dry-run-plan.json" {
    order = ["api"];
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
        destination = "registry.ix.dev/example/api:latest";
        sourceInstallable = ".#api";
      };
      region = "us-west-1";
      ipv4 = false;
      snapshot = false;
      # Exercise declarative per-VM secret attachments through the dry-run
      # create path (no live call, so the names need not exist here).
      secrets = [
        {
          name = "github_token";
          target = {
            name = "GH_TOKEN";
            injectAs = "env";
          };
        }
      ];
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
      nativeBuildInputs = [package];
      strictDeps = true;
    }
    ''
      ix-fleet --plan ${dryRunPlan} up --skip-push --skip-health --dry-run
      mkdir -p "$out"
    '';

  # Two remote-source nodes that share a build VM, so `switch --dry-run` exercises
  # the native multi-VM batch path: both must land in one `ix apply .#web .#worker
  # --build-vm builder` command.
  dryRunSwitchPlan = jsonFormat.generate "ix-fleet-dry-run-switch-plan.json" {
    order = [
      "web"
      "worker"
    ];
    nodes = lib.genAttrs ["web" "worker"] (name: {
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
        destination = "registry.ix.dev/example/${name}:latest";
        sourceInstallable = ".#${name}";
      };
      region = "us-west-1";
      ipv4 = false;
      snapshot = false;
    });
  };

  # `status --dry-run` reports desired state straight from the plan (image,
  # region, declared checks) without touching the API, so it runs in the
  # sandbox and pins the shape the fleet wrapper's `status` verb rides on.
  dryRunStatus =
    pkgs.runCommand "ix-fleet-dry-run-status"
    {
      nativeBuildInputs = [package];
      strictDeps = true;
    }
    ''
      ix-fleet --plan ${dryRunPlan} status --dry-run | tee status.log
      grep -qF '+ status api: image=registry.ix.dev/example/api:latest region=us-west-1 checks=-' status.log \
        || { echo "expected the desired-state status line for api" >&2; exit 1; }
      mkdir -p "$out"
    '';

  # `logs --dry-run` prints the journalctl argv it would exec per node.
  dryRunLogs =
    pkgs.runCommand "ix-fleet-dry-run-logs"
    {
      nativeBuildInputs = [package];
      strictDeps = true;
    }
    ''
      ix-fleet --plan ${dryRunPlan} logs --unit nginx.service --lines 7 --dry-run | tee logs.log
      grep -qF '+ logs api: exec journalctl --no-pager -n 7 -u nginx.service' logs.log \
        || { echo "expected the journalctl exec line for api" >&2; exit 1; }
      mkdir -p "$out"
    '';

  # Four replicas of one base node declaring updateStrategy.maxUnavailable = 2.
  rollingPlan = jsonFormat.generate "ix-fleet-rolling-plan.json" {
    order = map (index: "api-${toString index}") (lib.range 0 3);
    nodes = lib.genAttrs' (lib.range 0 3) (index: {
      name = "api-${toString index}";
      value = {
        name = "api-${toString index}";
        baseName = "api";
        replicaIndex = index;
        system = "/nix/store/api-system";
        switch = {
          target = "/nix/store/api-system";
          sourceInstallable = ".#api";
        };
        bootstrapImage = "registry.ix.dev/ix/base:latest";
        replacementImage = {
          imageName = "api";
          destination = "registry.ix.dev/example/api:latest";
          sourceInstallable = ".#api";
        };
        region = "us-west-1";
        ipv4 = false;
        snapshot = false;
        updateStrategy.maxUnavailable = 2;
      };
    });
  };

  # A live (non-dry-run) `up` hands its whole fan-out to IX_FLEET_DAG_RUNNER
  # before making any API call, so pointing that at a spec-capturing stub lets
  # the sandbox assert the rolling-update serialization edges: with
  # maxUnavailable = 2 over four replicas, replica i must depend on replica
  # i-2 and the first window must stay unconstrained. --skip-push keeps the
  # shared-destination image-push chain from adding its own edges on top.
  rollingUpdateDag =
    pkgs.runCommand "ix-fleet-rolling-update-dag"
    {
      nativeBuildInputs = [package pkgs.jq];
      strictDeps = true;
    }
    ''
      export IX_FLEET_DAG_RUNNER=${lib.getExe (ix.writeBashApplication pkgs {
        name = "capture-dag-spec";
        text = ''
          cp "$1" dag-spec.json
        '';
      })}
      ix-fleet --plan ${rollingPlan} up --skip-push --skip-health
      jq -e '
        (.nodes | length == 4)
        and (.nodes["api-0"].depends_on == [])
        and (.nodes["api-1"].depends_on == [])
        and (.nodes["api-2"].depends_on == ["api-0"])
        and (.nodes["api-3"].depends_on == ["api-1"])
      ' dag-spec.json \
        || { echo "rolling-update edges did not match the expected sliding window" >&2; exit 1; }
      mkdir -p "$out"
    '';

  # Walks the `switch` command's --dry-run control flow and asserts the two
  # batchable nodes collapse into one native multi-VM `ix apply` invocation rather
  # than one per node. No API calls or network, so it runs in the sandbox.
  dryRunSwitch =
    pkgs.runCommand "ix-fleet-dry-run-switch"
    {
      nativeBuildInputs = [package];
      strictDeps = true;
    }
    ''
      ix-fleet --plan ${dryRunSwitchPlan} switch --skip-health --no-snapshot --dry-run | tee switch.log
      grep -qE '\+ ix apply \.#web \.#worker --build-vm builder' switch.log \
        || { echo "expected a single batched 'ix apply .#web .#worker --build-vm builder'" >&2; exit 1; }
      mkdir -p "$out"
    '';

  package = unwrapped.overrideAttrs (old: {
    postInstall = ''
      # shell
      ${old.postInstall or ""}
      # Drop the prebuilt ix_sdk wheel into the venv site-packages so `import
      # ix_sdk` resolves both at runtime and for the ty install check, without a
      # PYTHONPATH shim. The cdylib comes from R2 (packages/ix-sdk-python).
      cp -r ${ixSdk}/${pkgs.python3.sitePackages}/. "$out/venv/${pkgs.python3.sitePackages}/"

      # --set-default (not --set) so tests can point IX_FLEET_DAG_RUNNER at a
      # spec-capturing stub; real invocations never set the variable and get
      # the pinned dag-runner.
      wrapProgram "$out/bin/ix-fleet" \
        --set-default IX_FLEET_DAG_RUNNER ${lib.escapeShellArg (lib.getExe dagRunner)}
    '';

    passthru =
      (old.passthru or {})
      // {
        tests =
          (old.passthru.tests or {})
          // {
            inherit dryRunUp dryRunSwitch dryRunStatus dryRunLogs rollingUpdateDag;
          };
      };
  });
in
  package
