# The forkable dev spec (RFC 0007). This is the one file a user edits after
# `ix dev init`. It is a plain attrset, not a NixOS module: `env` is the
# per-VM environment (a module), while `fleet` / `shared` / `selfSource` are
# fleet-level and must not be re-evaluated per node.
{
  # 1) ENVIRONMENT — layered on the base image on every node (single or fleet).
  env =
    { pkgs, ... }:
    {
      environment.systemPackages = [
        pkgs.ripgrep
        pkgs.jq
      ];
      programs.git.enable = true;
    };

  # Base image. development-base already ships our wrapped claude-code + codex,
  # so a fork gets the agents from a plain flake import. (Default; shown here
  # for the example.)
  baseImage = "development-base";

  # 2) FLEET — omit for a single default VM. Two interchangeable agents plus a
  # builder that opts out of the shared volume below.
  fleet.nodes = {
    agent.replicas = 2;
    builder.dependsOn = [ "agent" ];
  };

  # 3) SHARED SMB IDENTITY VOLUME — one Claude (and ix) login for the fleet.
  shared = {
    enable = true;
    mountPoint = "/shared";
    claudeAuth = true; # bind ~/.claude onto the share: one login, all agents
    ixAuth = true; # bind ~/.n onto the share: agents can spawn more VMs
    excludeNodes = [ "builder" ]; # per-VM opt-out; default is every node in
  };

  # Materialize /ix (this source) on every node so a VM can `ix up` more VMs
  # from the same spec. On the share when sharing is on, else a local copy.
  selfSource = true;
}
