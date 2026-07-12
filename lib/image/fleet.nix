/**
Colmena-style fleet evaluation for ix images.

Curried: the outer function takes the build dependencies (`lib`,
`evalImageConfig`, and `bootstrapImage`); the inner takes a fleet spec
(`defaults`, `deployment`, `nodes`) and returns the
declarative fleet plan and evaluated NixOS configurations.
*/
{
  lib,
  evalImageConfig,
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

  deploymentDefaults = {
    bootstrapImage = "registry.ix.dev/${bootstrapImage.name}:${bootstrapImage.tag}";
    region = "us-west-1";
    ipv4 = false;
    snapshot = true;
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
  # and `ix up` always waits for every declared check, so there is no
  # per-deployment selector.
  knownDeploymentKeys = [
    "bootstrapImage"
    "env"
    "ipv4"
    "l7ProxyPorts"
    "region"
    "secrets"
    "snapshot"
  ];
  checkedDeployment = name: deploy: let
    unknown = lib.subtractLists knownDeploymentKeys (attrNames deploy);
  in
    assert lib.assertMsg (!(elem "healthChecks" unknown)) ''
      fleet node '${name}' sets deployment.healthChecks, but health checks are not selected per deployment:
        declare checks as `ix.healthChecks.<name>` in one of the node's modules (service modules
        such as minecraft and nginx already declare theirs), and `ix up` waits for every
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
    "updateStrategy"
  ];

  # `updateStrategy` bounds how many of a node's replicas `ix up` updates
  # concurrently (Kubernetes RollingUpdate semantics): with
  # `maxUnavailable = k`, replica `i` waits for replica `i - k` to finish its
  # activation and health gates, so at most `k` replicas are unavailable at
  # once. A failed activation or health check halts the rollout before the
  # remaining replicas change. Default (null) converges every replica in
  # parallel.
  knownUpdateStrategyKeys = ["maxUnavailable"];
  checkedUpdateStrategy = name: strategy:
    if strategy == null
    then null
    else let
      unknown = lib.subtractLists knownUpdateStrategyKeys (attrNames (
        assert lib.assertMsg (isAttrs strategy)
        "fleet node '${name}': updateStrategy must be an attrset like { maxUnavailable = 1; }"; strategy
      ));
    in
      assert lib.assertMsg (unknown == []) ''
        fleet node '${name}' updateStrategy has unknown option(s): ${lib.concatStringsSep ", " unknown}
          valid options: ${lib.concatStringsSep ", " knownUpdateStrategyKeys}
      '';
      assert lib.assertMsg (
        isInt (strategy.maxUnavailable or null) && strategy.maxUnavailable > 0
      )
      "fleet node '${name}': updateStrategy.maxUnavailable must be a positive integer"; {
        inherit (strategy) maxUnavailable;
      };

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
    updateStrategy = checkedUpdateStrategy name (spec.updateStrategy or null);
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
        deploy = spec.deployment;
        ipv4HealthChecks = lib.filterAttrs (_: check: check.requiresIpv4) config.ix.healthChecks;
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
          inherit (deploy) bootstrapImage;
          inherit (deploy) region;
          inherit (deploy) ipv4;
          inherit (deploy) snapshot;
          inherit (spec) tags;
          groups = nodeGroups;
          inherit (deploy) env;
          inherit (deploy) l7ProxyPorts;
          # Per-VM user-store secret references plus delivery targets. `ix up`
          # verifies the source keys exist before deploying.
          secrets = normalizeSecrets (deploy.secrets or {});
          dependsOn = expandedDependencies.${name};
          healthChecks = planHealthChecks config;
          # Rolling-update window for replicas sharing `baseName`.
          inherit (spec) updateStrategy;
        }
    )
    checkedNodeSpecs;

  planValue = {
    order = attrNames checkedNodeSpecs;
    nodes = nodePlan;
  };

  # `ix up` derives each build from `nixosConfigurations.<node>` and realizes
  # it on that same target VM. No local closure or builder VM is part of the
  # fleet authoring contract.
  nixosConfigurations = lib.mapAttrs (_name: config: {inherit config;}) nodeConfigs;
in {
  inherit planValue nixosConfigurations;
  nodes = nodeConfigs;
  meta = checkedNodeSpecs;
}
