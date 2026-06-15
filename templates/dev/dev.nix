# Your ix dev environment (RFC 0007). This is the one file you edit; commit it
# to your own repo and fork it however you like. It is a plain attrset:
# `env` is the per-VM environment (a NixOS module); `fleet` / `shared` /
# `selfSource` are fleet-level.
{
  # Packages, dotfiles, languages, services — applied to every VM this spec
  # builds (single VM or fleet).
  env =
    { pkgs, ... }:
    {
      environment.systemPackages = [
        pkgs.ripgrep
        pkgs.jq
      ];
      programs.git.enable = true;
    };

  # development-base ships our wrapped claude-code + codex, so you get the
  # agents from a plain flake import. (This is the default; shown for clarity.)
  baseImage = "development-base";

  # Turn this into a fleet instead of a single VM by declaring nodes:
  #
  # fleet.nodes = {
  #   agent.replicas = 3;
  #   builder.dependsOn = [ "agent" ];
  # };

  # Give the fleet ONE shared Claude (and ix) login over an SMB volume. Off by
  # default: a plain fleet has no shared mount. When enabled it is on for every
  # node; list nodes in `excludeNodes` to opt them out.
  #
  # shared = {
  #   enable = true;
  #   mountPoint = "/shared";
  #   claudeAuth = true;   # bind ~/.claude onto the share: one login, all VMs
  #   ixAuth = true;       # bind ~/.n onto the share: VMs can spawn more VMs
  #   # excludeNodes = [ "builder" ];
  # };

  # Materialize /ix (this source) on every node so a VM can `ix up` more VMs
  # from the same spec.
  selfSource = true;
}
