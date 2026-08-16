# A node that can fetch and push a private GitHub repository using a token
# from the ix account secret store, and nothing else.
{
  config,
  lib,
  pkgs,
  ...
}: let
  # The fleet maps the account store key `github_token` to this runtime file.
  # Only the path is known at eval time. The token bytes live in the file ix
  # writes when creating the VM, and are read per git request.
  tokenPath = "/run/secrets/github/token";

  inherit (config.programs.git-token-auth) helperCommand;
in {
  # One option is the whole wiring: a credential helper registered in the
  # system git config for github.com, plus `insteadOf` rules so an SSH-form
  # remote reaches the same token. Helpers are additive across scopes, so a
  # user's `~/.config/git/config` can add its own without replacing this one.
  #
  # The helper is `ix-credential token-helper`, which speaks git's wire
  # protocol directly and never renders `https://user:token@host` into a
  # file. That matters because `@ : / %` are structural in a URL and ordinary
  # bytes in git's format, so a token containing one would otherwise produce
  # a credentials file matching nothing, and the failure would surface as a
  # 404 rather than as an authentication error.
  programs.git-token-auth.tokenFile = tokenPath;

  # `gh` does not use git's credential helper; it reads `GH_TOKEN` (or
  # `GITHUB_TOKEN`). It is left out of the global environment on purpose: an
  # exported token is visible in that process's `/proc/<pid>/environ`, is
  # inherited by every descendant, and can land in a core dump. Operators who
  # want the `gh` CLI authenticated point it at the same file per shell, e.g.
  #   export GH_TOKEN="$(cat /run/secrets/github/token)"
  # See the README for why this is not baked in.

  # Secret-independent: this asserts THIS example's wiring, not a returned
  # token, so it passes in CI and on a fresh boot with no token delivered.
  #
  # `--get <name> <value-pattern>` rather than reading the value back and
  # comparing, because helpers are additive: a user's global config could
  # shadow ours, and a plain read would return whichever has priority. An
  # anchored pattern over the exact string nix rendered is the honest check
  # that our helper is the registered one.
  ix.healthChecks.github-credential-helper = {
    description = "git is wired to the synced-token credential helper";
    command = [
      (lib.getExe pkgs.git)
      "config"
      "--get"
      "credential.https://github.com.helper"
      "^${lib.escapeRegex helperCommand}$"
    ];
  };
}
