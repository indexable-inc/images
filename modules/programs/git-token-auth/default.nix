# Authenticate git to a forge from a token file the host already holds.
#
# The case this exists for: a VM the ix secret store delivered a token to,
# which then has to clone, fetch and push against a private repository. Three
# copies of a 40-line bash credential helper had grown across this repo for
# exactly that (examples/synced-github/auth, users/harivansh-afk/dev,
# services.git-clone), each reimplementing git's wire protocol and each
# inventing its own failure signaling. `ix-credential token-helper` is that
# protocol, in Rust, with tests; this module is the wiring.
#
# Reading the token at request time, rather than rendering
# `https://user:token@host` into a credentials file, is the property worth
# keeping: `@ : / %` are structural in a URL and ordinary bytes in git's wire
# format, so a token containing one produces a credentials file matching
# nothing, and the failure surfaces as a 404 on a private repository.
{
  config,
  lib,
  pkgs,
  ...
}: let
  inherit (lib) mkIf mkOption types;
  cfg = config.programs.git-token-auth;

  tool = lib.getExe pkgs.ix-credential;
  hostArgs = lib.concatMapStringsSep " " (host: "--allow-host ${host}") cfg.hosts;
  helper = "${tool} token-helper --token-file ${cfg.tokenFile} --username ${cfg.username} ${hostArgs}";

  # An `insteadOf` pair per host, so an SSH-form remote reaches the same
  # token rather than needing a deploy key.
  sshRewrites = lib.genAttrs' cfg.hosts (host:
    lib.nameValuePair "url \"https://${host}/\"" {
      insteadOf = [
        "git@${host}:"
        "ssh://git@${host}/"
      ];
    });

  # A helper entry per host alongside the plain-scope one. Both, because the
  # two consumers disagree: git prefers the narrowest matching scope, and
  # gitoxide's url-scoped helper support is the less-traveled path. The
  # helper re-checks the request's host itself, so the broad registration
  # still answers for `hosts` only.
  hostScopes =
    lib.genAttrs' cfg.hosts (host:
      lib.nameValuePair "credential \"https://${host}\"" {inherit helper;});
in {
  options.programs.git-token-auth = {
    tokenFile = mkOption {
      type = types.nullOr types.str;
      default = null;
      example = "/run/secrets/github/token";
      description = ''
        Runtime path to a file holding the token. Typically where a
        {option}`deployment.secrets` entry materializes. Setting it is what
        turns this module on; the path is read per git request, so it need
        not exist at build time or at boot.

        A token is a password, not a URL component: it is handed to git over
        its own wire protocol and never rendered into a remote URL.
      '';
    };

    hosts = mkOption {
      type = types.listOf types.str;
      default = ["github.com"];
      description = ''
        Forge hosts the token answers for. The helper re-checks the host on
        every request, so a token cannot leave for a host outside this list
        even when something other than git invokes the helper.
      '';
    };

    username = mkOption {
      type = types.str;
      default = "x-access-token";
      description = ''
        The username paired with the token. GitHub ignores it for PATs and
        app tokens but the field must be present.
      '';
    };

    helperCommand = mkOption {
      type = types.str;
      readOnly = true;
      description = ''
        The credential-helper command this module registers with git.
        Read-only and derived, so a health check or a test can assert
        against the same string git was configured with rather than
        restating it.
      '';
    };

    preflight = mkOption {
      type = types.str;
      readOnly = true;
      description = ''
        A command that exits nonzero, naming the cause, when the token file
        cannot produce a credential. Read-only and derived: put it in the
        {option}`serviceConfig.ExecStartPre` of a unit about to need the
        token, so a missing secret fails with a message instead of letting a
        private repository answer 404, which reads as a wrong URL.
      '';
    };
  };

  config = mkIf (cfg.tokenFile != null) {
    programs = {
      git-token-auth = {
        helperCommand = helper;
        preflight = "${tool} token-check --token-file ${cfg.tokenFile}";
      };

      # The credential config needs somewhere to land, and a host holding a
      # checkout wants git anyway. mkDefault so a consumer can still say no.
      git = {
        enable = lib.mkDefault true;
        config =
          {credential.helper = helper;}
          // hostScopes
          // sshRewrites;
      };
    };
  };
}
