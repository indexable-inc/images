# First-class, cross-platform DNS sinkhole. `networking.blockedHosts` is the one
# declarative list of base domains to hard-route to 127.0.0.1, rendered once into
# `networking.blockedHostsText`. Each host imports the platform branch that
# applies it the idiomatic way: NixOS via the first-class `networking.extraHosts`
# (default.nix), nix-darwin via the /etc/hosts activation script it has
# no module for (darwin.nix). Both branches import this file, so the
# option lives in exactly one place.
#
# The module ships no blocklist data on purpose: which domains to sinkhole is
# user policy, so the list always lives in the consumer flake.
{
  lib,
  config,
  ...
}: {
  options.networking.blockedHosts = lib.mkOption {
    type = lib.types.listOf lib.types.str;
    default = [];
    example = ["tiktok.com"];
    description = ''
      Base domains hard-routed to 127.0.0.1 in /etc/hosts (a local DNS
      sinkhole). Both the apex domain and its `www.` host are blocked.
    '';
  };

  # Internal: the rendered sinkhole lines, computed once and consumed by the
  # platform branch so the two never drift.
  options.networking.blockedHostsText = lib.mkOption {
    type = lib.types.lines;
    internal = true;
    readOnly = true;
    description = "Rendered 127.0.0.1 lines for `networking.blockedHosts`.";
  };

  config.networking.blockedHostsText = lib.concatLines (
    lib.concatMap (host: [
      "127.0.0.1\t${host}"
      "127.0.0.1\twww.${host}"
    ])
    config.networking.blockedHosts
  );
}
