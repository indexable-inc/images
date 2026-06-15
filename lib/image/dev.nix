/**
  `mkDev`: an opinionated dev-fleet layer over `mkFleet` (RFC 0007).

  Consumes one user-owned NixOS module (the forkable `dev.nix`) and returns the
  same result shape `mkFleet` does (`.up`, `.health`, `.diff`, `withNodePrefix`,
  …), so it drops straight into the flake/example plumbing.

  The user's `dev.nix` is an ordinary NixOS module: `environment.systemPackages`
  and friends at the top level, plus `ix.dev.*` (see `lib/dev/options.nix`) to
  describe the agents, fleet, and shared volume. `mkDev` reads `ix.dev` once via
  a probe eval to plan the fleet, then builds each node with the same module:

  - `ix.dev.baseImage` + the agent layer + the user's module become `mkFleet`
    `defaults`, so every node is the user's environment and ships the agents.
  - `ix.dev.fleet` becomes `mkFleet` `nodes` (a single `dev` node if empty).
  - `ix.dev.shared.enable` synthesizes a dedicated `file-server` node running
    `smbd` and injects the CIFS client + identity-bind modules into every node
    not in `excludeNodes`, on a private east-west group so the share is never
    public. The dedicated server decouples the canonical credentials' lifecycle
    from the workload VMs.
  - `ix.dev.selfSource` materializes `/ix` (the dev source) on every node, on
    the volume when one exists, else a local writable copy.

  Curried `mkDevFor hostSystem { module, src }` so flake/example evaluation can
  build the wrapper derivations for the requested system, mirroring `mkFleetFor`.
  `src` is the flake `self`, threaded in by the template's `flake.nix`; it is
  what gets materialized at `/ix`. The user's `dev.nix` never mentions it.
*/
{
  lib,
  paths,
  mkFleetFor,
  evalImageConfig,
}:
let
  inherit (import ../dev/shared-mount.nix { inherit lib; }) serverModule clientModule;
  inherit (import ../dev/identity.nix { inherit lib; }) bindModule sourceNode sourceServerSeed;

  # Plain option/agent modules (paths, resolved relative to this file).
  agentsModule = ../dev/agents.nix;

  # On-disk export path on the elected server, and the internal SMB share name.
  shareDir = "/var/lib/ix-dev-share";
  shareName = "dev";

  mkDevFor =
    hostSystem:
    {
      module,
      src ? null,
    }:
    let
      # Read `ix.dev` without forcing the per-node environment. Reuses the real
      # eval path (the agent layer pulls in the option declarations), so the
      # topology is read the same way it will later be built. Forcing `ix.dev`
      # is cheap: it does not evaluate `environment.systemPackages` or build the
      # agent wrapper.
      dev =
        (evalImageConfig {
          modules = [
            agentsModule
            module
          ];
        }).ix.dev;

      shared = dev.shared;
      sharedEnable = shared.enable;
      inherit (shared)
        mountPoint
        excludeNodes
        group
        guestOk
        ;
      serverNode = shared.server;

      fleetNodes = if dev.fleet == { } then { dev = { }; } else dev.fleet;

      binds =
        (lib.optional shared.claude {
          localPath = "/root/.claude";
          shareSubdir = "claude";
        })
        ++ (lib.optional shared.ix {
          localPath = "/root/.n";
          shareSubdir = "n";
        });

      haveSource = dev.selfSource && src != null;
      onShare = sharedEnable && haveSource;
      shareSubdirs = map (b: b.shareSubdir) binds ++ lib.optional onShare "ix";

      # `defaults` apply to EVERY node (workload and server): the base image, the
      # ix.dev options + agent layer, and the user's module. The mkForce on the
      # image name overrides the base image's own plain `ix.image.name` so each
      # node's replacement image is named after the node, not the base.
      defaults = [
        (paths.images + "/dev/${dev.baseImage}")
        agentsModule
        (
          { name, ... }:
          {
            ix.image.name = lib.mkForce name;
            ix.image.tag = lib.mkDefault "ix-dev";
          }
        )
        module
      ];

      nodeIncluded = name: !(builtins.elem name excludeNodes);

      # Per-workload-node dev modules. Applied to the base node spec, so every
      # replica inherits them.
      workloadModules =
        name:
        lib.optionals (sharedEnable && nodeIncluded name) (
          [
            (clientModule {
              inherit serverNode shareName mountPoint;
              guest = guestOk;
            })
          ]
          ++ lib.optional (binds != [ ]) (bindModule {
            inherit mountPoint binds;
          })
        )
        ++ lib.optional haveSource (sourceNode {
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

      serverSpec.${serverNode} = {
        groups = [ group ];
        modules = [
          (serverModule {
            inherit shareName shareDir guestOk;
            subdirs = shareSubdirs;
          })
        ]
        ++ lib.optional onShare (sourceServerSeed {
          inherit src shareDir;
        });
      };

      nodes = augmentedNodes // lib.optionalAttrs sharedEnable serverSpec;
    in
    (mkFleetFor hostSystem) { inherit defaults nodes; };
in
{
  inherit mkDevFor;
}
