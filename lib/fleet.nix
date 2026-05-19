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
    region = "hil-1";
    ipv4 = false;
    snapshot = true;
    switch.buildOn = "remote";
  };

  mergeDeployments =
    parts:
    lib.mergeAttrsList parts
    // {
      env = lib.mergeAttrsList (map (part: part.env or { }) parts);
      l7ProxyPorts = lib.unique (lib.concatMap (part: part.l7ProxyPorts or [ ]) parts);
    };

  isWrappedNode = value: builtins.isAttrs value && (value ? module || value ? modules);

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
    in
    {
      inherit name;
      modules = asList defaults ++ moduleList spec;
      tags = lib.unique (asList (spec.tags or [ ]));
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

  rawNodeSpecs = lib.mapAttrs normalizeNode nodes;
  nodeSpecs = lib.mergeAttrsList (lib.mapAttrsToList expandReplicas rawNodeSpecs);
  expandDependency =
    dep:
    if builtins.hasAttr dep rawNodeSpecs then
      if rawNodeSpecs.${dep}.replicas == 1 then
        [ dep ]
      else
        lib.genList (index: "${dep}-${toString index}") rawNodeSpecs.${dep}.replicas
    else
      [ dep ];

  nodeConfigs = lib.mapAttrs (
    name: spec:
    evalImageConfig {
      modules = [
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
  ) nodeSpecs;

  nodeRefs = lib.mapAttrs (_name: config: { inherit config; }) nodeConfigs;
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
      inherit (deploy) env;
      inherit (deploy) l7ProxyPorts;
      dependsOn = lib.concatMap expandDependency spec.dependsOn;
      healthChecks = planHealthChecks config;
    }
  ) nodeSpecs;

  planValue = {
    order = builtins.attrNames nodeSpecs;
    nodes = nodePlan;
    inherit secrets;
  };

  plan = pkgs.writeText "ix-fleet-plan.json" (builtins.toJSON planValue);
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
    "diff"
    "health"
    "replace"
    "switch"
    "up"
  ] mkFleetCmd;

in
{
  inherit (subcommands)
    diff
    replace
    health
    switch
    up
    ;
  command = mkFleetCmd null;
  planCommand = mkFleetCmd "plan";

  inherit plan planValue;
  nodes = nodeConfigs;
  meta = nodeSpecs;
  packages = lib.mapAttrs (_: config: config.ix.build.ociImage) nodeConfigs;
  systemPackages = lib.mapAttrs' (
    name: config: lib.nameValuePair "${name}-system" config.system.build.toplevel
  ) nodeConfigs;
}
