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
  # Peer VMs evaluated elsewhere, merged into the `nodes` module argument
  # so cross-VM references (`ix.endpointOf nodes.<peer> ...`) work when
  # VMs are wired together one `mkVm` at a time instead of in one fleet
  # spec (ix#8306). Values are `{ config }` wrappers, i.e. exactly the
  # entries of another result's `nixosConfigurations`. Own nodes win on
  # name collisions.
  peers ? {},
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

  # Default `switch.sourceInstallable`. The remote path goes through `ix apply`,
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

  knownSourceKeys = ["path" "sourceId" "destination" "activateServices"];
  normalizeSourceDestination = destination:
    "/"
    + lib.concatStringsSep "/" (
      lib.filter (
        component: component != "" && component != "."
      ) (lib.splitString "/" destination)
    );

  normalizeSourceAttachment = name: value:
    assert lib.assertMsg (isSecretName name)
    "source key '${name}' must be lower snake_case: [a-z][a-z0-9_]*";
    assert lib.assertMsg (isAttrs value) "source '${name}' must be an attrset";
    assert lib.assertMsg (
      lib.subtractLists knownSourceKeys (attrNames value) == []
    ) "source '${name}' has unknown option(s): ${lib.concatStringsSep ", " (lib.subtractLists knownSourceKeys (attrNames value))}; valid options: ${lib.concatStringsSep ", " knownSourceKeys}";
    assert lib.assertMsg (
      (value ? path) != (value ? sourceId)
    ) "source '${name}' must set exactly one of path or sourceId";
    assert lib.assertMsg (
      !(value ? path)
      || (
        builtins.isString value.path
        && value.path != ""
        && builtins.match "/.*" value.path == null
        && !(builtins.elem ".." (lib.splitString "/" value.path))
        && (
          let
            components = lib.filter (component: component != "" && component != ".") (lib.splitString "/" value.path);
          in
            components
            != []
            && components != [".ix"]
        )
      )
    ) "source '${name}'.path must name a dedicated artifact subtree below the apply source root (not . or .ix), not a Nix path";
    assert lib.assertMsg (
      !(value ? sourceId)
      || (
        builtins.isString value.sourceId
        && builtins.match "[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}" value.sourceId != null
      )
    ) "source '${name}'.sourceId must be a UUID string";
    assert lib.assertMsg (
      value ? destination
      && builtins.isString value.destination
      && builtins.match "/.+" value.destination != null
      && !(builtins.elem ".." (lib.splitString "/" value.destination))
      && normalizeSourceDestination value.destination != "/"
      && lib.all (
        protected:
          normalizeSourceDestination value.destination
          != protected
          && !(lib.hasPrefix "${protected}/" (normalizeSourceDestination value.destination))
      ) ["/dev" "/proc" "/sys" "/nix/store"]
    ) "source '${name}'.destination must be an absolute normalized guest path outside protected system trees";
    assert lib.assertMsg (
      builtins.isList (value.activateServices or [])
      && lib.all (
        service:
          builtins.isString service
          && builtins.match "[A-Za-z0-9@_.:-]+" service != null
          && !(lib.hasSuffix ".service" service)
      ) (value.activateServices or [])
    ) "source '${name}'.activateServices must contain systemd service names without the .service suffix"; {
      inherit name;
      path = value.path or null;
      sourceId = value.sourceId or null;
      destination = normalizeSourceDestination value.destination;
      activateServices = lib.unique (value.activateServices or []);
    };

  normalizeSources = sources: let
    normalized = lib.mapAttrsToList normalizeSourceAttachment sources;
    destinations = map (source: source.destination) normalized;
    hasNestedDestinations =
      lib.any (
        outer:
          lib.any (
            inner:
              outer
              != inner
              && (
                lib.hasPrefix "${outer}/" inner
                || lib.hasPrefix "${inner}/" outer
              )
          )
          destinations
      )
      destinations;
  in
    assert lib.assertMsg (
      lib.length destinations == lib.length (lib.unique destinations)
    ) "deployment.sources entries must use distinct destination paths";
    assert lib.assertMsg (!hasNestedDestinations)
    "deployment.sources entries must not use nested destination paths"; normalized;

  /**
  Embed the post-switch Source contract in the guest system.

  `ix apply` cannot evaluate deployment metadata on the caller: local applies
  deliberately upload the source and evaluate on the VM so the caller needs no
  Nix installation. The activated closure therefore exposes one small JSON
  manifest under `/etc/ix/`; after the ordinary system switch, `ix apply` reads
  it through guest exec, uploads/reuses the declared artifacts, materializes
  them, and invokes the fixed activator below.

  Services named by `activateServices` are fail-closed behind a runtime marker.
  A boot or switch removes the marker and stops an old instance; only the
  post-materialization activator recreates it and starts the services. This
  closes the `ConditionPathExists` trap where a unit skipped during boot stays
  skipped after its artifact appears.
  */
  sourceRuntimeModule = sources: {
    config,
    lib,
    pkgs,
    ...
  }: let
    activateServices = lib.unique (lib.concatMap (source: source.activateServices) sources);
    serviceUnits = map (service: "${service}.service") activateServices;
    sourceDestinations = map (source: source.destination) sources;
    hasStartCommand = value:
      if builtins.isString value
      then value != ""
      else if builtins.isList value
      then value != []
      else value != null;
    serviceIsStartable = service: let
      unit = config.systemd.services.${service};
    in
      hasStartCommand (unit.script or null)
      || hasStartCommand (unit.serviceConfig.ExecStart or null);
    systemctl = lib.getExe' config.systemd.package "systemctl";
    readyPath = "/run/ix/sources-ready";
    activator = pkgs.writeShellApplication {
      name = "ix-source-activate";
      runtimeInputs = [
        pkgs.coreutils
        config.systemd.package
      ];
      text = ''
        install -d -m 0755 /run/ix
        touch ${readyPath}
        ${lib.optionalString (serviceUnits != []) ''
          systemctl reset-failed ${lib.escapeShellArgs serviceUnits} || true
          if ! systemctl start ${lib.escapeShellArgs serviceUnits}; then
            systemctl stop ${lib.escapeShellArgs serviceUnits} || true
            rm -f ${readyPath}
            exit 1
          fi
        ''}
      '';
    };
    stager = pkgs.writeShellApplication {
      name = "ix-source-stage";
      runtimeInputs = [
        pkgs.coreutils
        pkgs.util-linux
      ];
      text = ''
        if [[ "$#" -ne 3 ]]; then
          echo "usage: ix-source-stage <prepare|commit|abort> <destination> <token>" >&2
          exit 2
        fi
        operation="$1"
        destination="$2"
        token="$3"
        case "$destination" in
          ${lib.concatMapStringsSep "\n          " (destination: "${lib.escapeShellArg destination}) ;;") sourceDestinations}
          *)
            echo "ix-source-stage: undeclared destination: $destination" >&2
            exit 2
            ;;
        esac
        if [[ ! "$token" =~ ^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$ ]]; then
          echo "ix-source-stage: invalid apply token" >&2
          exit 2
        fi

        next="$destination.ix-source-$token.next"
        install -d -m 0755 /run/ix

        case "$operation" in
          prepare)
            install -d -m 0755 "$(dirname -- "$next")"
            mkdir -- "$next"
            ;;
          abort)
            rm -rf -- "$next"
            ;;
          commit)
            exec 9>/run/ix/source-stage.lock
            flock 9
            if [[ ! -d "$next" || -L "$next" ]]; then
              echo "ix-source-stage: staged tree is missing or not a directory" >&2
              exit 1
            fi
            if [[ -e "$destination" || -L "$destination" ]]; then
              # `--exchange --no-copy` maps to renameat2(RENAME_EXCHANGE):
              # the old and new trees swap atomically on the same filesystem,
              # with no missing-destination window or copy fallback.
              mv -T --exchange --no-copy -- "$next" "$destination"
              rm -rf -- "$next"
            else
              mv -T --no-copy -- "$next" "$destination"
            fi
            ;;
          *)
            echo "ix-source-stage: unknown operation: $operation" >&2
            exit 2
            ;;
        esac
      '';
    };
  in
    lib.mkIf (sources != []) {
      assertions =
        map (service: {
          assertion = serviceIsStartable service;
          message = "deployment.sources activateServices references missing or non-startable systemd service '${service}'";
        })
        activateServices;

      environment.etc."ix/deployment-sources.json".text = builtins.toJSON {
        version = 1;
        inherit sources;
      };
      environment.systemPackages = [
        activator
        stager
      ];

      system.activationScripts.ixSourceGate = {
        deps = ["etc"];
        text = ''
          ${lib.optionalString (serviceUnits != []) ''
            ${systemctl} stop ${lib.escapeShellArgs serviceUnits} || true
          ''}
          ${lib.getExe' pkgs.coreutils "rm"} -f ${readyPath}
        '';
      };

      systemd.services = lib.genAttrs activateServices (_service: {
        unitConfig.ConditionPathExists = [readyPath];
      });
    };

  mergeDeployments = parts:
    lib.mergeAttrsList parts
    // {
      env = lib.mergeAttrsList (map (part: part.env or {}) parts);
      l7ProxyPorts = lib.unique (lib.concatMap (part: part.l7ProxyPorts or []) parts);
      # User-store secret keys merge by source name; node layers can override a
      # fleet-wide delivery target while unrelated refs compose.
      secrets = lib.foldl' lib.recursiveUpdate {} (map (part: part.secrets or {}) parts);
      # Source keys merge like secrets, except the mutually-exclusive identity
      # selector is replaced as a pair: a node-level sourceId must remove an
      # inherited path (and vice versa).
      sources = lib.foldl' mergeSourceLayer {} (map (part: part.sources or {}) parts);
    };

  mergeSourceAttachment = previous: next:
    if !(isAttrs previous && isAttrs next)
    then next
    else let
      merged = lib.recursiveUpdate previous next;
    in
      if next ? path
      then builtins.removeAttrs merged ["sourceId"]
      else if next ? sourceId
      then builtins.removeAttrs merged ["path"]
      else merged;

  mergeSourceLayer = accumulated: layer:
    assert lib.assertMsg (isAttrs layer) "deployment.sources must be an attrset";
      lib.foldl' (
        result: name:
          result
          // {
            ${name} = mergeSourceAttachment (result.${name} or {}) layer.${name};
          }
      )
      accumulated
      (attrNames layer);

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
    "sources"
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
    "updateStrategy"
  ];

  # `updateStrategy` bounds how many of a node's replicas `ix-fleet up` /
  # `replace` recreate concurrently (Kubernetes RollingUpdate semantics): with
  # `maxUnavailable = k`, replica `i` waits for replica `i - k` to finish its
  # whole workflow -- recreate, boot, health checks -- so at most `k` replicas
  # are down at once and a failing health check halts the rollout before it
  # takes the remaining replicas down. Default (null) keeps today's behavior:
  # every replica converges in parallel.
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

  nodeSources =
    lib.mapAttrs (
      _name: spec: normalizeSources (spec.deployment.sources or {})
    )
    checkedNodeSpecs;

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
              (sourceRuntimeModule nodeSources.${name})
            ]
            ++ spec.modules;
        }
    )
    checkedNodeSpecs;

  nodeRefs = peers // lib.mapAttrs (_name: config: {inherit config;}) nodeConfigs;
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
        # ix apply expects a system out-path for local copy and a .drv for remote
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
            # `ix apply` can derive each VM name from the attr; local switches keep the
            # `.#<node>-system` package alias (see `defaultSourceInstallable`).
            sourceInstallable =
              deploy.switch.sourceInstallable or (defaultSourceInstallable name switchBuildOn);
            overrideInputs = deploy.switch.overrideInputs or {};
          };
          inherit (deploy) bootstrapImage;
          # The plan carries only the `.#<node>` installable string (the
          # fleet's `packages.<node>` attr, the node's CAS image): ix-fleet
          # `nix build`s it at push time, mirroring `switch.sourceInstallable`.
          # Never put the image's outPath/drvPath here: the CAS manifest
          # builder reads its closure at eval (IFD), so forcing either would
          # build every node's system closure just to render this plan.
          replacementImage = {
            inherit imageName;
            destination = replacementDestination;
            sourceInstallable = ".#${name}";
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
          # Public attachment metadata only. The deprecated image `up` /
          # `replace` path uses this to reject Source-bearing nodes before any
          # mutation; source-aware switching is owned by `ix apply`.
          sources = nodeSources.${name};
          dependsOn = expandedDependencies.${name};
          healthChecks = planHealthChecks config;
          # Rolling-update window for this node's replica group; ix-fleet
          # turns it into serialization edges among replicas sharing
          # `baseName` (see checkedUpdateStrategy above).
          inherit (spec) updateStrategy;
        }
    )
    checkedNodeSpecs;

  planValue = {
    order = attrNames checkedNodeSpecs;
    nodes = nodePlan;
  };

  # Rename a fleet's external identities without re-evaluating any NixOS
  # closure: only plan data (node names, `dependsOn`, east-west `groups`, the
  # registry `destination` the replacement image is pushed to, and the two
  # installable attrs) carries the prefix, while `system`/`switch` `target`
  # keep pointing at the shared base closures. The prefixed installables
  # (`.#${prefix}${name}`) still resolve to the shared base closure because
  # `nixosConfigurations.<external>` and `packages.<external>` are thin
  # renames over the once-evaluated `nodeConfigs.<name>`
  # (see `resultFor`), so the native multi-VM `ix apply` can name the prefixed VM
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
                  # Always re-derived: unlike `switch.sourceInstallable` there
                  # is no user-facing override, the installable is defined as
                  # this fleet's `packages.<external>` attr.
                  sourceInstallable = ".#${prefixName name}";
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
        "logs"
        "replace"
        "status"
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
      logs
      status
      switch
      up
      ;
    command = mkFleetCmd null;
    planCommand = mkFleetCmd "plan";

    inherit plan;
    planValue = planValueFor;
    nodes = externalKeyed nodeConfigs;
    meta = externalKeyed checkedNodeSpecs;
    # Each node's CAS-manifest image, the target of the plan's
    # `replacementImage.sourceInstallable`; merge into a flake's top-level
    # `packages` so `nix build .#<node>` resolves it. Lazy: forcing one
    # requires the ix-side `ix.build.casImageBuilder` module (cas-layer.nix).
    packages = externalKeyed (lib.mapAttrs (_: config: config.ix.build.casImage) nodeConfigs);
    systemPackages =
      lib.mapAttrs' (
        name: config: lib.nameValuePair "${externalName name}-system" config.system.build.toplevel
      )
      nodeConfigs;
    # Each node's NixOS system under its bare external name, so `ix apply .#<node>`
    # (and the native multi-VM `ix apply .#a .#b --build-vm <builder>`) resolves
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
