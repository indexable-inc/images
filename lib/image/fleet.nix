/**
  Colmena-style fleet evaluation for ix images.

  Curried: the outer function takes the build dependencies (`lib`,
  `pkgs`, `evalImageConfig`, the `ix fleet` script, and the Nushell
  application helper); the inner takes a fleet spec
  (`defaults`, `deployment`, `secrets`, `nodes`) and returns the
  rendered fleet plan, image attrset, and wrapped CLI app.
*/
{
  lib,
  pkgs,
  secretsLib,
  evalImageConfig,
  ixFleet,
  writeNushellApplication,
  bootstrapImage,
}:
{
  defaults ? [ ],
  deployment ? { },
  secrets ? { },
  nodes,
  # Prefix prepended to every node name and to every `dependsOn` string.
  # Lets a non-production consumer (test runner, scratch fleet) reuse an
  # example without colliding with real VMs that share the natural node
  # names. Defaults to empty so production callers see no change.
  nodePrefix ? "",
}:
let
  asList = value: if builtins.isList value then value else [ value ];

  moduleList =
    spec:
    if spec ? modules then
      asList spec.modules
    else if spec ? module then
      asList spec.module
    else
      [ ];

  deploymentDefaults = {
    bootstrapImage = "registry.ix.dev/${bootstrapImage.name}:${bootstrapImage.tag}";
    region = "us-west-1";
    ipv4 = false;
    snapshot = true;
    switch.buildOn = "remote";
  };
  secretSet = secretsLib.normalize secrets;
  secretRefs = secretSet.refs;

  mergeDeployments =
    parts:
    lib.mergeAttrsList parts
    // {
      env = lib.mergeAttrsList (map (part: part.env or { }) parts);
      l7ProxyPorts = lib.unique (lib.concatMap (part: part.l7ProxyPorts or [ ]) parts);
    };

  wrappedNodeKeys = [
    "module"
    "modules"
    "deployment"
    "tags"
    "groups"
    "dependsOn"
    "replicas"
  ];

  isWrappedNode =
    value: builtins.isAttrs value && lib.any (key: builtins.hasAttr key value) wrappedNodeKeys;

  prefixExternalName = name: nodePrefix + name;
  prefixWrappedNode =
    spec:
    if !(isWrappedNode spec) then
      spec
    else
      spec
      // lib.optionalAttrs (spec ? dependsOn) {
        dependsOn = map prefixExternalName (asList spec.dependsOn);
      }
      // lib.optionalAttrs (spec ? groups) {
        groups = map prefixExternalName (asList spec.groups);
      };
  prefixedNodes =
    if nodePrefix == "" then
      nodes
    else
      lib.mapAttrs' (
        name: spec: lib.nameValuePair (prefixExternalName name) (prefixWrappedNode spec)
      ) nodes;

  normalizeNode =
    name: value:
    let
      spec = if isWrappedNode value then value else { modules = [ value ]; };
      deploymentParts = [
        deploymentDefaults
        deployment
      ]
      ++ [
        (spec.deployment or { })
      ];
      groups = asList (spec.groups or [ ]);
    in
    {
      inherit name;
      modules = asList defaults ++ moduleList spec;
      tags = lib.unique (asList (spec.tags or [ ]));
      groups = lib.unique groups;
      deployment = mergeDeployments deploymentParts;
      dependsOn = asList (spec.dependsOn or [ ]);
      replicas = spec.replicas or 1;
    };

  expandReplicas =
    name: spec:
    assert lib.assertMsg (
      builtins.isInt spec.replicas && spec.replicas > 0
    ) "fleet node '${name}': replicas must be a positive integer";
    if spec.replicas == 1 then
      {
        ${name} = spec // {
          baseName = name;
        };
      }
    else
      builtins.listToAttrs (
        lib.genList (index: {
          name = "${name}-${toString index}";
          value = spec // {
            name = "${name}-${toString index}";
            baseName = name;
            replicaIndex = index;
          };
        }) spec.replicas
      );

  rawNodeSpecs = lib.mapAttrs normalizeNode prefixedNodes;
  nodeSpecs = lib.mergeAttrsList (lib.mapAttrsToList expandReplicas rawNodeSpecs);
  knownDependency = dep: builtins.hasAttr dep rawNodeSpecs || builtins.hasAttr dep nodeSpecs;
  unknownDependencies = lib.filterAttrs (_: deps: deps != [ ]) (
    lib.mapAttrs (_name: spec: lib.filter (dep: !(knownDependency dep)) spec.dependsOn) rawNodeSpecs
  );
  renderUnknownDependencies = name: deps: "${name}: ${lib.concatStringsSep ", " deps}";
  checkedKnownNodeSpecs =
    assert lib.assertMsg (unknownDependencies == { }) ''
      fleet nodes reference unknown dependencies:
        ${lib.concatStringsSep "\n  " (lib.mapAttrsToList renderUnknownDependencies unknownDependencies)}
    '';
    nodeSpecs;
  expandDependency =
    dep:
    if builtins.hasAttr dep rawNodeSpecs then
      if rawNodeSpecs.${dep}.replicas == 1 then
        [ dep ]
      else
        lib.genList (index: "${dep}-${toString index}") rawNodeSpecs.${dep}.replicas
    else
      [ dep ];
  expandedDependencies = lib.mapAttrs (
    _name: spec: lib.unique (lib.concatMap expandDependency spec.dependsOn)
  ) checkedKnownNodeSpecs;
  cycleFromPath =
    target: path:
    let
      go =
        remaining:
        if remaining == [ ] then
          [ target ]
        else if builtins.head remaining == target then
          remaining ++ [ target ]
        else
          go (builtins.tail remaining);
    in
    go path;
  detectDependencyCycle =
    dependencies:
    let
      visit =
        path: name:
        if builtins.elem name path then
          cycleFromPath name path
        else
          let
            cycles = lib.filter (cycle: cycle != [ ]) (
              map (dep: visit (path ++ [ name ]) dep) dependencies.${name}
            );
          in
          if cycles == [ ] then [ ] else builtins.head cycles;
      cycles = lib.filter (cycle: cycle != [ ]) (
        map (name: visit [ ] name) (builtins.attrNames dependencies)
      );
    in
    if cycles == [ ] then [ ] else builtins.head cycles;
  dependencyCycle = detectDependencyCycle expandedDependencies;
  checkedNodeSpecs =
    assert lib.assertMsg (dependencyCycle == [ ]) ''
      fleet nodes contain a dependency cycle:
        ${lib.concatStringsSep " -> " dependencyCycle}
    '';
    checkedKnownNodeSpecs;

  nodeConfigs = lib.mapAttrs (
    name: spec:
    evalImageConfig {
      modules = [
        {
          _module.args = {
            inherit name;
            nodes = nodeRefs;
            fleet.nodes = nodeRefs;
            inherit secretRefs;
            fleet.secretRefs = secretRefs;
          };

          ix.image.name = lib.mkDefault name;
          networking.hostName = lib.mkDefault name;
        }
      ]
      ++ spec.modules;
    }
  ) checkedNodeSpecs;

  # Module-args `nodes` is keyed by the example's base node names so cross-node
  # references like `nodes.file-server.config.ix.networking.eastWest.hostName`
  # keep working when the fleet was rebuilt with a `nodePrefix`. The prefix is
  # an external (VM-name / image-name / hostname) concern; it must not change
  # how an example refers to its own siblings.
  nodeRefs = lib.mapAttrs' (
    name: config: lib.nameValuePair (lib.removePrefix nodePrefix name) { inherit config; }
  ) nodeConfigs;
  planHealthChecks =
    config:
    lib.mapAttrs (_name: check: {
      inherit (check)
        attempts
        description
        from
        intervalSec
        requiresIpv4
        timeoutSec
        ;
      command = map builtins.unsafeDiscardStringContext check.command;
    }) config.ix.healthChecks;

  nodePlan = lib.mapAttrs (
    name: spec:
    let
      config = nodeConfigs.${name};
      imageName = config.ix.image.name;
      imageTag = config.ix.image.tag;
      deploy = spec.deployment;
      replacementDestination = deploy.destination or "${imageName}:${imageTag}";
      switchBuildOn = deploy.switch.buildOn or "remote";
      ipv4HealthChecks = lib.filterAttrs (_: check: check.requiresIpv4) config.ix.healthChecks;
      # ix switch expects a system out-path for local copy and a .drv for remote
      # build. Picking the wrong shape uploads the build-time closure and tries
      # to run `<drv>/bin/switch-to-configuration`, which deadlocks.
      switchTarget = deploy.switch.target or builtins.unsafeDiscardStringContext (
        if switchBuildOn == "local" then
          "${config.system.build.toplevel}"
        else
          config.system.build.toplevel.drvPath
      );
    in
    assert lib.assertMsg (deploy.ipv4 || ipv4HealthChecks == { })
      "fleet node '${name}' has health checks that require deployment.ipv4 = true: ${lib.concatStringsSep ", " (lib.attrNames ipv4HealthChecks)}";
    {
      inherit
        name
        ;
      inherit (spec) baseName;
      replicaIndex = spec.replicaIndex or null;
      system = builtins.unsafeDiscardStringContext "${config.system.build.toplevel}";
      switch = {
        target = switchTarget;
        buildOn = switchBuildOn;
        buildVm = deploy.switch.buildVm or null;
        sourceInstallable = deploy.switch.sourceInstallable or ".#${name}-system";
        overrideInputs = deploy.switch.overrideInputs or { };
      };
      inherit (deploy) bootstrapImage;
      replacementImage = {
        inherit
          imageName
          imageTag
          ;
        destination = replacementDestination;
        source = builtins.unsafeDiscardStringContext "${config.ix.build.ociImage}";
        sourceDrv = builtins.unsafeDiscardStringContext config.ix.build.ociImage.drvPath;
      };
      inherit (deploy) region;
      inherit (deploy) ipv4;
      inherit (deploy) snapshot;
      recreateOnUp = deploy.recreateOnUp or false;
      inherit (spec) tags;
      inherit (spec) groups;
      inherit (deploy) env;
      inherit (deploy) l7ProxyPorts;
      dependsOn = expandedDependencies.${name};
      healthChecks = planHealthChecks config;
    }
  ) checkedNodeSpecs;

  planValue = {
    order = builtins.attrNames checkedNodeSpecs;
    nodes = nodePlan;
    secrets = secretSet.plan;
  };

  plan = (pkgs.formats.json { }).generate "ix-fleet-plan.json" planValue;
  userLocalBinPath = ''
    let home = ($env.HOME? | default "")
    if $home != "" {
      $env.PATH = [$"($home)/.local/bin"] ++ $env.PATH
    }
  '';
  # Wraps `ix-fleet [sub]` with a stable PATH that includes ~/.local/bin so
  # users see their installed `ix` binary, not whatever nix happens to find.
  mkFleetCmd =
    sub:
    writeNushellApplication pkgs {
      name = if sub == null then "ix-fleet" else "ix-fleet-${sub}";
      runtimeInputs = [ ixFleet ];
      text = ''
        def --wrapped main [...args] {
          ${userLocalBinPath}
          exec ${lib.getExe ixFleet} --plan ${plan} ${lib.optionalString (sub != null) "${sub} "}...$args
        }
      '';
    };

  subcommands = lib.genAttrs [
    "bootstrap"
    "diff"
    "down"
    "health"
    "replace"
    "switch"
    "up"
  ] mkFleetCmd;

in
{
  inherit (subcommands)
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

  inherit plan planValue;
  nodes = nodeConfigs;
  meta = checkedNodeSpecs;
  packages = lib.mapAttrs (_: config: config.ix.build.ociImage) nodeConfigs;
  systemPackages = lib.mapAttrs' (
    name: config: lib.nameValuePair "${name}-system" config.system.build.toplevel
  ) nodeConfigs;
}
