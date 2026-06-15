/**
  `mkDev`: an opinionated dev-fleet layer over `mkFleet` (RFC 0007).

  Consumes one user-owned spec (the forkable `dev.nix`) and produces the same
  result shape `mkFleet` does (`.up`, `.health`, `.diff`, `withNodePrefix`, …),
  so it drops straight into the example/flake plumbing. It performs four
  transforms and configures nothing itself beyond wiring the dev modules:

  - `env` + `baseImage` become `mkFleet` `defaults`, so every node is the
    user's environment. The default base is `development-base`, which already
    ships our wrapped `claude-code` and `codex`; that is how a fork gets the
    agents from a plain flake import.
  - `fleet.nodes` becomes `mkFleet` `nodes` (a single `dev` node if absent), so
    the same spec yields either one default VM or a whole fleet.
  - `shared.enable` synthesizes a dedicated `file-server` node running `smbd`
    and injects the CIFS client + identity-bind modules into every node not in
    `excludeNodes`, joining the fleet to a private east-west group so the share
    is never public. The dedicated server keeps the canonical credentials'
    lifecycle decoupled from the workload VMs.
  - `selfSource` materializes `/ix` (the dev source) on every node for
    recursion, on the shared volume when one exists, else a local writable copy.

  Curried `mkDevFor hostSystem spec` so example/flake evaluation can build the
  wrapper derivations for the requested system, mirroring `mkFleetFor`.
*/
{
  lib,
  paths,
  mkFleetFor,
}:
let
  inherit (import ../dev/shared-mount.nix { inherit lib; }) serverModule clientModule;
  inherit (import ../dev/shared-auth.nix { inherit lib; }) authModule;
  inherit (import ../dev/self-source.nix { inherit lib; }) nodeModule serverSeedModule;

  # On-disk export path on the elected server.
  shareDir = "/var/lib/ix-dev-share";

  mkDevFor =
    hostSystem: spec:
    let
      baseImage = spec.baseImage or "development-base";
      tag = spec.tag or "ix-dev";
      env = spec.env or { };
      selfSource = spec.selfSource or true;
      # The flake `self` source, threaded in by the template's flake.nix. Null
      # when a caller cannot provide it (then `/ix` is simply not materialized).
      src = spec.src or null;

      shared = spec.shared or { };
      sharedEnable = shared.enable or false;
      mountPoint = shared.mountPoint or "/shared";
      shareName = shared.shareName or "dev";
      serverNode = shared.server or "file-server";
      claudeAuth = shared.claudeAuth or false;
      ixAuth = shared.ixAuth or false;
      excludeNodes = shared.excludeNodes or [ ];
      group = shared.group or "ix-dev-shared";
      guestOk = shared.guestOk or true;

      # Workload topology: the user's fleet, or one node named `dev`.
      fleetNodes = (spec.fleet or { }).nodes or { dev = { }; };

      # Identity dirs to bind onto the share.
      binds =
        (lib.optional claudeAuth {
          localPath = "/root/.claude";
          shareSubdir = "claude";
        })
        ++ (lib.optional ixAuth {
          localPath = "/root/.n";
          shareSubdir = "n";
        });

      haveSource = selfSource && src != null;
      onShare = sharedEnable && haveSource;
      shareSubdirs = map (b: b.shareSubdir) binds ++ lib.optional onShare "ix";

      # `defaults` apply to EVERY node (workload and server). The mkForce on the
      # image name overrides the base image's own plain `ix.image.name` so each
      # node's replacement image is named after the node, not the base.
      imageDefaults = [
        (paths.images + "/dev/${baseImage}")
        (
          { name, ... }:
          {
            ix.image.name = lib.mkForce name;
            ix.image.tag = lib.mkDefault tag;
          }
        )
        env
      ];

      nodeIncluded = name: !(builtins.elem name excludeNodes);

      # Per-workload-node dev modules. Applied to the base node spec, so every
      # replica inherits them.
      workloadModules =
        name:
        lib.optionals (sharedEnable && nodeIncluded name) (
          [
            (clientModule {
              inherit
                serverNode
                shareName
                mountPoint
                ;
              guest = guestOk;
            })
          ]
          ++ lib.optional (binds != [ ]) (authModule {
            inherit mountPoint binds;
          })
        )
        ++ lib.optional haveSource (nodeModule {
          inherit src mountPoint;
          onShare = onShare && nodeIncluded name;
        });

      asNodeSpec = value: if builtins.isAttrs value then value else { modules = [ value ]; };

      augmentedNodes = lib.mapAttrs (
        name: value:
        let
          nspec = asNodeSpec value;
        in
        nspec
        // {
          modules = (nspec.modules or [ ]) ++ workloadModules name;
          groups = (nspec.groups or [ ]) ++ lib.optional (sharedEnable && nodeIncluded name) group;
          # Only nodes that actually mount the share wait for the server.
          dependsOn = (nspec.dependsOn or [ ]) ++ lib.optional (sharedEnable && nodeIncluded name) serverNode;
        }
      ) fleetNodes;

      serverSpec = {
        ${serverNode} = {
          groups = [ group ];
          modules = [
            (serverModule {
              inherit shareName shareDir guestOk;
              subdirs = shareSubdirs;
            })
          ]
          ++ lib.optional onShare (serverSeedModule {
            inherit src shareDir;
          });
        };
      };

      nodes = augmentedNodes // lib.optionalAttrs sharedEnable serverSpec;
    in
    (mkFleetFor hostSystem) {
      defaults = imageDefaults;
      inherit nodes;
    };
in
{
  inherit mkDevFor;
}
