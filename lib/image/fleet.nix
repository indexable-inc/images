/**
Colmena-style fleet evaluation for ix images.

Curried: the outer function takes the build dependencies (`lib`,
`pkgs`, `evalImageConfig`, the `ix fleet` script, and the Nushell
application helper); the inner takes a fleet spec
(`defaults`, `deployment`, `nodes`) and returns the
rendered fleet plan, image attrset, and wrapped CLI app.
*/
{
  lib,
  pkgs,
  evalImageConfig,
  ixFleet,
  writeNushellApplication,
  bootstrapImage,
}: {
  defaults ? [],
  deployment ? {},
  nodes,
}: let
  inherit
    (builtins)
    attrNames
    elem
    filter
    hasAttr
    isAttrs
    isInt
    unsafeDiscardStringContext
    ;

  inherit (lib) toList;

  moduleList = spec: toList (spec.modules or spec.module or []);

  # Default `switch.sourceInstallable`. The remote path goes through `ix up`,
  # which rewrites a bare `.#<node>` to `nixosConfigurations.<node>...` and (for
  # the native multi-VM switch) derives the VM name from that attr. The local
  # path runs a plain `nix build <installable>` with no such rewrite, so it must
  # name the `.#<node>-system` package alias that resolves to the toplevel.
  defaultSourceInstallable = nodeName: buildOn:
    if buildOn == "local"
    then ".#${nodeName}-system"
    else ".#${nodeName}";

  deploymentDefaults = {
    bootstrapImage = "registry.ix.dev/${bootstrapImage.name}:${bootstrapImage.tag}";
    region = "us-west-1";
    ipv4 = false;
    snapshot = true;
    switch.buildOn = "remote";
  };
  isSecretName = name: builtins.match "[a-z][a-z0-9_]*" name != null;

  normalizeSecretAttachment = sourceName: value:
    assert lib.assertMsg (isSecretName sourceName)
    "secret key '${sourceName}' must be lower snake_case: [a-z][a-z0-9_]*";
    assert lib.assertMsg (isAttrs value) "secret '${sourceName}' must be an attrset";
      if value ? env
      then
        assert lib.assertMsg (!(value ? file)) "secret '${sourceName}' cannot set both env and file"; {
          name = sourceName;
          target = {
            name = value.env;
            injectAs = "env";
          };
        }
      else if value ? file
      then {
        name = sourceName;
        target =
          {
            name = value.file;
            injectAs = "file";
          }
          // lib.optionalAttrs (value ? owner) {inherit (value) owner;}
          // lib.optionalAttrs (value ? mode) {inherit (value) mode;};
      }
      else throw "secret '${sourceName}' must set either env or file";

  normalizeSecrets = secrets: lib.mapAttrsToList normalizeSecretAttachment secrets;

  mergeDeployments = parts:
    lib.mergeAttrsList parts
    // {
      env = lib.mergeAttrsList (map (part: part.env or {}) parts);
      l7ProxyPorts = lib.unique (lib.concatMap (part: part.l7ProxyPorts or []) parts);
      # User-store secret keys merge by source name; node layers can override a
      # fleet-wide delivery target while unrelated refs compose.
      secrets = lib.foldl' lib.recursiveUpdate {} (map (part: part.secrets or {}) parts);
    };

  # Every deployment key the plan consumes. `deployment` is a plain attrset
  # (not a NixOS module), so a typo or an imagined option would otherwise be
  # merged and silently dropped. `healthChecks` gets a dedicated message
  # because examples historically wrote `deployment.healthChecks = [ ... ]`
  # as if it selected checks to wait for: checks are declared by the node's
  # modules via `ix.healthChecks.<name>` (with `from`, `command`, retries)
  # and `ix-fleet up` always waits for every declared check, so there is no
  # per-deployment selector.
  knownDeploymentKeys = [
    "bootstrapImage"
    "destination"
    "env"
    "ipv4"
    "l7ProxyPorts"
    "recreateOnUp"
    "region"
    "secrets"
    "snapshot"
    "switch"
  ];
  checkedDeployment = name: deploy: let
    unknown = lib.subtractLists knownDeploymentKeys (attrNames deploy);
  in
    assert lib.assertMsg (!(elem "healthChecks" unknown)) ''
      fleet node '${name}' sets deployment.healthChecks, but health checks are not selected per deployment:
        declare checks as `ix.healthChecks.<name>` in one of the node's modules (service modules
        such as minecraft and nginx already declare theirs), and `ix-fleet up` waits for every
        declared check. Remove deployment.healthChecks; there is no allowlist to configure.
    '';
    assert lib.assertMsg (unknown == []) ''
      fleet node '${name}' deployment has unknown option(s): ${lib.concatStringsSep ", " unknown}
        valid options: ${lib.concatStringsSep ", " knownDeploymentKeys}
    ''; deploy;

  wrappedNodeKeys = [
    "module"
    "modules"
    "deployment"
    "tags"
    "groups"
    "dependsOn"
    "replicas"
  ];

  isWrappedNode = value: isAttrs value && lib.any (key: value ? "${key}") wrappedNodeKeys;

  normalizeNode = name: value: let
    spec =
      if isWrappedNode value
      then value
      else {modules = [value];};
    deploymentParts =
      [
        deploymentDefaults
        deployment
      ]
      ++ [
        (spec.deployment or {})
      ];
    groups = toList (spec.groups or []);
  in {
    inherit name;
    modules = toList defaults ++ moduleList spec;
    tags = lib.unique (toList (spec.tags or []));
    groups = lib.unique groups;
    deployment = checkedDeployment name (mergeDeployments deploymentParts);
    dependsOn = toList (spec.dependsOn or []);
    replicas = spec.replicas or 1;
  };

  expandReplicas = name: spec:
    assert lib.assertMsg (
      isInt spec.replicas && spec.replicas > 0
    ) "fleet node '${name}': replicas must be a positive integer";
      if spec.replicas == 1
      then {
        ${name} =
          spec
          // {
            baseName = name;
          };
      }
      else
        lib.listToAttrs (
          lib.genList (
            index:
              lib.nameValuePair "${name}-${toString index}" (
                spec
                // {
                  name = "${name}-${toString index}";
                  baseName = name;
                  replicaIndex = index;
                }
              )
          )
          spec.replicas
        );

  rawNodeSpecs = lib.mapAttrs normalizeNode nodes;
  nodeSpecs = lib.concatMapAttrs expandReplicas rawNodeSpecs;
  knownDependency = dep: hasAttr dep rawNodeSpecs || hasAttr dep nodeSpecs;
  unknownDependencies = lib.filterAttrs (_: deps: deps != []) (
    lib.mapAttrs (_name: spec: filter (dep: !(knownDependency dep)) spec.dependsOn) rawNodeSpecs
  );
  renderUnknownDependencies = name: deps: "${name}: ${lib.concatStringsSep ", " deps}";
  checkedKnownNodeSpecs = assert lib.assertMsg (unknownDependencies == {}) ''
    fleet nodes reference unknown dependencies:
      ${lib.concatMapAttrsStringSep "\n  " renderUnknownDependencies unknownDependencies}
  ''; nodeSpecs;
  expandDependency = dep:
    if hasAttr dep rawNodeSpecs
    then
      if rawNodeSpecs.${dep}.replicas == 1
      then [dep]
      else lib.genList (index: "${dep}-${toString index}") rawNodeSpecs.${dep}.replicas
    else [dep];
  expandedDependencies =
    lib.mapAttrs (
      _name: spec: lib.unique (lib.concatMap expandDependency spec.dependsOn)
    )
    checkedKnownNodeSpecs;
  # `before a b` holds when a must be ordered before b, i.e. b depends on a.
  # toposort returns `{ result = … }` when acyclic and `{ cycle; loops; }` otherwise.
  dependencyOrder = lib.toposort (a: b: elem a expandedDependencies.${b}) (
    attrNames expandedDependencies
  );
  checkedNodeSpecs = assert lib.assertMsg (dependencyOrder ? result) ''
    fleet nodes contain a dependency cycle:
      ${lib.concatStringsSep " -> " (dependencyOrder.cycle or [])}
  ''; checkedKnownNodeSpecs;

  nodeConfigs =
    lib.mapAttrs (
      name: spec:
        evalImageConfig {
          modules =
            [
              {
                _module.args = {
                  inherit name;
                  nodes = nodeRefs;
                  fleet.nodes = nodeRefs;
                };

                ix.image.name = lib.mkDefault name;
                networking.hostName = lib.mkDefault name;
              }
            ]
            ++ spec.modules;
        }
    )
    checkedNodeSpecs;

  nodeRefs = lib.mapAttrs (_name: config: {inherit config;}) nodeConfigs;
  planHealthChecks = config:
    lib.mapAttrs (_name: check: {
      inherit
        (check)
        attempts
        description
        from
        intervalSec
        requiresIpv4
        timeoutSec
        ;
      command = map unsafeDiscardStringContext check.command;
    })
    config.ix.healthChecks;

  nodePlan =
    lib.mapAttrs (
      name: spec: let
        config = nodeConfigs.${name};
        imageName = config.ix.image.name;
        deploy = spec.deployment;
        replacementDestination = deploy.destination or "${imageName}:latest";
        switchBuildOn = deploy.switch.buildOn or "remote";
        ipv4HealthChecks = lib.filterAttrs (_: check: check.requiresIpv4) config.ix.healthChecks;
        # ix up expects a system out-path for local copy and a .drv for remote
        # build. Picking the wrong shape uploads the build-time closure and tries
        # to run `<drv>/bin/switch-to-configuration`, which deadlocks.
        switchTarget = deploy.switch.target or unsafeDiscardStringContext (
          if switchBuildOn == "local"
          then "${config.system.build.toplevel}"
          else config.system.build.toplevel.drvPath
        );
        # Image-declared membership (`ix.networking.groups`) unions with the
        # fleet-level `nodes.<name>.groups`: the image carries its own network
        # identity, the fleet adds deployment-specific memberships on top.
        nodeGroups = lib.unique (spec.groups ++ config.ix.networking.groups);
        # Mirrors the server's validate_group_slug rule (63 = the DNS label
        # octet limit) so a bad slug fails the eval, not the create RPC
        # mid-deploy.
        invalidGroups = filter (slug: builtins.match "[a-z0-9_-]{1,63}" slug == null) nodeGroups;
      in
        assert lib.assertMsg (deploy.ipv4 || ipv4HealthChecks == {})
        "fleet node '${name}' has health checks that require deployment.ipv4 = true: ${lib.concatStringsSep ", " (lib.attrNames ipv4HealthChecks)}";
        assert lib.assertMsg (invalidGroups == [])
        "fleet node '${name}' has invalid east-west group slug(s) (allowed: [a-z0-9_-], max 63 chars): ${lib.concatStringsSep ", " invalidGroups}"; {
          inherit
            name
            ;
          inherit (spec) baseName;
          replicaIndex = spec.replicaIndex or null;
          system = unsafeDiscardStringContext "${config.system.build.toplevel}";
          switch = {
            target = switchTarget;
            buildOn = switchBuildOn;
            buildVm = deploy.switch.buildVm or null;
            # Remote switches default to the bare `.#<node>` so the native multi-VM
            # `ix up` can derive each VM name from the attr; local switches keep the
            # `.#<node>-system` package alias (see `defaultSourceInstallable`).
            sourceInstallable =
              deploy.switch.sourceInstallable or (defaultSourceInstallable name switchBuildOn);
            overrideInputs = deploy.switch.overrideInputs or {};
          };
          inherit (deploy) bootstrapImage;
          replacementImage = {
            inherit imageName;
            destination = replacementDestination;
            source = unsafeDiscardStringContext "${config.ix.build.ociImage}";
            sourceDrv = unsafeDiscardStringContext config.ix.build.ociImage.drvPath;
          };
          inherit (deploy) region;
          inherit (deploy) ipv4;
          inherit (deploy) snapshot;
          recreateOnUp = deploy.recreateOnUp or false;
          inherit (spec) tags;
          groups = nodeGroups;
          inherit (deploy) env;
          inherit (deploy) l7ProxyPorts;
          # Per-VM user-store secret references plus delivery targets. ix-fleet
          # verifies the source keys exist before deploying.
          secrets = normalizeSecrets (deploy.secrets or {});
          dependsOn = expandedDependencies.${name};
          healthChecks = planHealthChecks config;
        }
    )
    checkedNodeSpecs;

  planValue = {
    order = attrNames checkedNodeSpecs;
    nodes = nodePlan;
  };

  # Rename a fleet's external identities without re-evaluating any NixOS
  # closure: only plan data (node names, `dependsOn`, east-west `groups`, the
  # registry `destination` the replacement image is pushed to, and the default
  # `switch.sourceInstallable` attr) carries the prefix, while `system`/`switch`
  # `target` and the OCI image `source`/`sourceDrv` keep pointing at the shared
  # base closures. The prefixed `sourceInstallable` (`.#${prefix}${name}`) still
  # resolves to the shared base closure because `nixosConfigurations.<external>`
  # is a thin `{ config }` wrapper over the once-evaluated `nodeConfigs.<name>`
  # (see `resultFor`), so the native multi-VM `ix up` can name the prefixed VM
  # without a second eval. The health-check
  # runner relies on this so the 10 example fleets are evaluated once per
  # `nix flake check`/`.#packages` eval instead of twice (ENG-2411). The
  # guest-side identity (`networking.hostName`, `ix.image.name`) therefore
  # stays base-named; the safety property the prefix exists for (lifecycle
  # scripts only ever force-delete VMs named after plan nodes, e.g.
  # `health-check-*`) lives entirely in the plan names.
  prefixedPlanValue = prefix: let
    prefixName = name: prefix + name;
  in {
    order = map prefixName planValue.order;
    nodes =
      lib.mapAttrs' (
        name: node:
          lib.nameValuePair (prefixName name) (
            node
            // {
              name = prefixName name;
              baseName = prefixName node.baseName;
              dependsOn = map prefixName node.dependsOn;
              groups = map prefixName node.groups;
              replacementImage =
                node.replacementImage
                // {
                  destination = prefixName node.replacementImage.destination;
                };
              # Re-derive only the default installable to the prefixed attr, keyed
              # on whether the user set `switch.sourceInstallable` in the spec (not
              # on the rendered string, which an explicit `.#<node>` override would
              # match). An explicit installable points at a real flake attr and is
              # left untouched.
              switch =
                node.switch
                // lib.optionalAttrs (!((checkedNodeSpecs.${name}.deployment.switch or {}) ? sourceInstallable)) {
                  sourceInstallable = defaultSourceInstallable (prefixName name) node.switch.buildOn;
                };
            }
          )
      )
      planValue.nodes;
  };

  userLocalBinPath = ''
    let home = ($env.HOME? | default "")
    if $home != "" {
      $env.PATH = [$"($home)/.local/bin"] ++ $env.PATH
    }
  '';

  resultFor = prefix: let
    externalName = name: prefix + name;
    externalKeyed = lib.mapAttrs' (name: value: lib.nameValuePair (externalName name) value);
    planValueFor =
      if prefix == ""
      then planValue
      else prefixedPlanValue prefix;
    plan = (pkgs.formats.json {}).generate "ix-fleet-plan.json" planValueFor;
    # Wraps `ix-fleet [sub]` with a stable PATH that includes ~/.local/bin so
    # users see their installed `ix` binary, not whatever nix happens to find.
    mkFleetCmd = sub:
      writeNushellApplication pkgs {
        name =
          if sub == null
          then "ix-fleet"
          else "ix-fleet-${sub}";
        runtimeInputs = [ixFleet];
        text = ''
          # nu
          def --wrapped main [...args] {
            ${userLocalBinPath}
            exec ${lib.getExe ixFleet} --plan ${plan} ${lib.optionalString (sub != null) "${sub} "}...$args
          }
        '';
      };

    subcommands =
      lib.genAttrs [
        "bootstrap"
        "diff"
        "down"
        "health"
        "replace"
        "switch"
        "up"
      ]
      mkFleetCmd;
  in {
    inherit
      (subcommands)
      bootstrap
      diff
      down
      replace
      health
      switch
      up
      ;
    command = mkFleetCmd null;
    planCommand = mkFleetCmd "plan";

    inherit plan;
    planValue = planValueFor;
    nodes = externalKeyed nodeConfigs;
    meta = externalKeyed checkedNodeSpecs;
    packages = externalKeyed (lib.mapAttrs (_: config: config.ix.build.ociImage) nodeConfigs);
    systemPackages =
      lib.mapAttrs' (
        name: config: lib.nameValuePair "${externalName name}-system" config.system.build.toplevel
      )
      nodeConfigs;
    # Each node's NixOS system under its bare external name, so `ix up .#<node>`
    # (and the native multi-VM `ix up .#a .#b --build-vm <builder>`) resolves
    # `nixosConfigurations.<node>.config.system.build.toplevel`. `nodeConfigs`
    # is already the evaluated `config` (`evalImageConfig` returns `.config`),
    # so the `{ config }` wrapper reuses that closure with no second eval; this
    # is the same closure `systemPackages.<node>-system` points at. Merge this
    # into a flake's top-level `nixosConfigurations`.
    nixosConfigurations = externalKeyed (lib.mapAttrs (_name: config: {inherit config;}) nodeConfigs);
    # Prepend `newPrefix` to every external name; the underlying NixOS
    # closures stay shared with the unprefixed fleet (see
    # `prefixedPlanValue` above).
    withNodePrefix = newPrefix: resultFor (newPrefix + prefix);
  };
in
  resultFor ""
