# Hari's dev VM: the ix environment he boots to do real work in, as opposed
# to `examples/dev/vm`, which is the generic teaching copy of RFC 0007.
#
# The environment is deliberately not restated here. It is
# `users/harivansh-afk/home.nix` -- the same home-manager module his laptop
# and hari-compute-1 consume -- so the VM cannot drift from his other
# machines. What this file owns is exactly what that module's header calls
# out as somebody else's job ("host/system concerns (accounts, sshd, mosh)
# belong to the consuming host config"): the account, the login surface, and
# the one credential the environment expects to find at runtime.
{
  ix,
  pkgs,
  ...
}: let
  username = "hari";

  # His laptop key (`~/.ssh/id_ed25519.pub`, comment `rathi@mac`). A public
  # key is not a secret, and committing it is what lets a fresh `ix apply`
  # come up already reachable: no key push step, no password auth to enable
  # "just for the first login".
  authorizedKey = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIM6tzq33IQcurWoQ7vhXOTLjv8YkdTGb7NoNsul3Sbfu rathi@mac";

  homeModule = import (ix.paths.users + "/harivansh-afk/home.nix") {inherit ix;};

  # The ix account secret store delivers `github_token` here (declared in
  # ./default.ix). Only the path is known at eval time; the bytes arrive when
  # the VM is created and never enter the store.
  tokenPath = "/run/secrets/github/token";
in {
  users.users.${username} = {
    isNormalUser = true;
    description = "Harivansh Rathi";
    # wheel for sudo. The home module configures zsh, which is only reached if
    # zsh is genuinely the login shell.
    extraGroups = ["wheel"];
    shell = pkgs.zsh;
    openssh.authorizedKeys.keys = [authorizedKey];
  };

  # The account has no password at all (key-only ssh below), so a sudo that
  # prompted would be a sudo that could never succeed.
  security.sudo.wheelNeedsPassword = false;

  # His actual environment. It lands on `hari` rather than root because
  # modules/profiles/base already owns `home-manager.users.root` and the two
  # configs define the same options with different concrete values (btop's
  # `color_theme` is "gotham" there and "ayu" here), which is an eval
  # conflict, not a merge. A separate account is also what the home module was
  # written against on hari-compute-1.
  home-manager.users.${username}.imports = [homeModule];

  services.openssh = {
    enable = true;
    settings = {
      # The VM is reachable through `ix shell` and, where the client has IPv6,
      # directly over its public IPv6 address: keys only, and never root.
      PasswordAuthentication = false;
      KbdInteractiveAuthentication = false;
      PermitRootLogin = "no";
    };
  };

  programs = {
    # NixOS only adds zsh to /etc/shells and installs its system rc when the
    # program module is on; without this the `shell` above is a failed login
    # rather than a shell.
    zsh.enable = true;

    # mosh is why sshd is here at all: the client sshs in only to start
    # `mosh-server`, and the session itself is UDP, so it survives roaming
    # between networks and a closed laptop lid. That is the difference between
    # a VM you visit and one you work in.
    #
    # `programs.mosh` rather than the package plus an `ix.networking.expose`
    # entry, which is the one place this config steps outside the port-claim
    # registry. mosh picks a port per session out of 60000-61000 and `expose`
    # has no spelling for a range, so claiming it would mean pinning a single
    # port -- and a pinned port has to be passed as `mosh --port=N`, which stops
    # the invocation being a bare `mosh <host>` and so stops the home module's
    # zsh shim from auto-attaching mux. One-command access is the whole point,
    # so the range wins. The trade-off is real and worth naming: nothing
    # eval-checks a future service that claims a port inside that range.
    mosh.enable = true;

    # Answers git's `get` for github.com from `tokenPath`, so a fetch or push
    # from this VM authenticates with no key push and no `gh auth login`.
    # Helpers are additive across scopes, so his own ~/.config/git/config (from
    # the home module, which leaves `programs.gh.gitCredentialHelper` on) adds
    # to this rather than replacing it, and this one answers whenever `gh`
    # itself has not been logged in.
    git-token-auth.tokenFile = tokenPath;
  };

  ix.networking = {
    expose = {
      # `firewall = false` because `services.openssh` opens 22 through its own
      # `openFirewall`; this entry is the port claim and the cross-node
      # discovery record, not a second opener.
      ssh = {
        port = 22;
        firewall = false;
        description = "ssh (mosh bootstrap)";
      };
    };
  };
}
