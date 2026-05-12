# Colmena-style fleet evaluation for ix images.
{
  lib,
  pkgs,
  evalImageConfig,
  ixFleetScript,
  writeNushellApplication,
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
    bootstrapImage = "registry.ix.dev/ix/test-cluster-bootstrap:zstd-tools-2026-05-12";
    region = "hil-1";
    ipv4 = false;
    snapshot = true;
    switch.buildOn = "remote";
  };

  mergeDeployments =
    parts:
    let
      merged = lib.foldl' (acc: part: acc // part) { } parts;
      env = lib.foldl' (acc: part: acc // (part.env or { })) { } parts;
      l7ProxyPorts = lib.unique (lib.concatMap (part: part.l7ProxyPorts or [ ]) parts);
    in
    merged // { inherit env l7ProxyPorts; };

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
  nodeSpecs = lib.foldl' (acc: name: acc // expandReplicas name rawNodeSpecs.${name}) { } (
    builtins.attrNames rawNodeSpecs
  );
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

  nodePlan = lib.mapAttrs (
    name: spec:
    let
      config = nodeConfigs.${name};
      imageName = config.ix.image.name;
      imageTag = config.ix.image.tag;
      deploy = spec.deployment;
      replacementDestination = deploy.destination or "${imageName}:${imageTag}";
      switchBuildOn = deploy.switch.buildOn or "remote";
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
      inherit (spec) tags;
      inherit (deploy) env;
      inherit (deploy) l7ProxyPorts;
      dependsOn = lib.concatMap expandDependency spec.dependsOn;
    }
  ) nodeSpecs;

  planValue = {
    order = builtins.attrNames nodeSpecs;
    nodes = nodePlan;
    inherit secrets;
  };

  plan = pkgs.writeText "ix-fleet-plan.json" (builtins.toJSON planValue);
  python = pkgs.python3.withPackages (ps: [ ps.pydantic ]);
  command = writeNushellApplication pkgs {
    name = "ix-fleet";
    runtimeInputs = [ python ];
    text = ''
      def --wrapped main [...args] {
        exec python3 ${ixFleetScript} --plan ${plan} ...$args
      }
    '';
  };
  planCommand = writeNushellApplication pkgs {
    name = "ix-fleet-plan";
    runtimeInputs = [ python ];
    text = ''
      def --wrapped main [...args] {
        exec python3 ${ixFleetScript} --plan ${plan} plan ...$args
      }
    '';
  };
  diff = writeNushellApplication pkgs {
    name = "ix-fleet-diff";
    runtimeInputs = [ python ];
    text = ''
      def --wrapped main [...args] {
        exec python3 ${ixFleetScript} --plan ${plan} diff ...$args
      }
    '';
  };
  switch = writeNushellApplication pkgs {
    name = "ix-fleet-switch";
    runtimeInputs = [ python ];
    text = ''
      def --wrapped main [...args] {
        exec python3 ${ixFleetScript} --plan ${plan} switch ...$args
      }
    '';
  };
  replace = writeNushellApplication pkgs {
    name = "ix-fleet-replace";
    runtimeInputs = [ python ];
    text = ''
      def --wrapped main [...args] {
        exec python3 ${ixFleetScript} --plan ${plan} replace ...$args
      }
    '';
  };
  up = writeNushellApplication pkgs {
    name = "ix-fleet-up";
    runtimeInputs = [ python ];
    text = ''
      def --wrapped main [...args] {
        exec python3 ${ixFleetScript} --plan ${plan} up ...$args
      }
    '';
  };

in
{
  inherit
    command
    diff
    plan
    planCommand
    replace
    switch
    up
    ;

  inherit planValue;
  nodes = nodeConfigs;
  meta = nodeSpecs;
  packages = lib.mapAttrs (_: config: config.ix.build.ociImage) nodeConfigs;
  systemPackages = lib.mapAttrs' (
    name: config: lib.nameValuePair "${name}-system" config.system.build.toplevel
  ) nodeConfigs;
}
