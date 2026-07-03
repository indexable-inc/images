{
  lib,
  pkgs,
  ...
}: let
  # The fleet maps the account store key `github_token` to this runtime file.
  # Only the path is known at eval time. The token bytes live in the file ix
  # writes when creating the VM.
  tokenPath = "/run/secrets/github/token";

  # A git credential helper that answers `get` for github.com with the token
  # read from `tokenPath` on demand. Three properties carry the design:
  #   1. The token never enters the store or any process environment. git
  #      reads it from the helper's stdout only when it actually needs a
  #      credential for a push or an authenticated fetch.
  #   2. If the file is absent (token not delivered yet) the helper exits 0
  #      with no output, so git falls through instead of failing. Boot and
  #      anonymous git operations never depend on the secret being present.
  #   3. It emits the token only for an `https`/`github.com` request. git
  #      already scopes this helper to that host via the
  #      `[credential "https://github.com"]` section, but re-checking the
  #      request git feeds on stdin means the token can never be handed to
  #      another host even if some non-git caller invokes the helper out of
  #      scope.
  # No external binaries: `[`, `printf`, `read`, `case`, and `$(<file)` are all
  # bash builtins, so the helper has no runtime PATH requirement.
  #
  # Kept as raw bash, not migrated to a checked writer: this is a perf-sensitive
  # git credential helper invoked per git operation, deliberately NOT `set -e`
  # (its control flow is built on intentional nonzero `|| exit 0` fall-throughs),
  # builtins-only with no PATH. A nushell rewrite adds startup latency and the
  # bash writer's `set -euo pipefail` would change its semantics.
  # astlog-ignore: no-write-shell-script
  credentialHelper = pkgs.writeShellScript "github-token-credential-helper" ''
    [ "$1" = get ] || exit 0
    [ -r ${lib.escapeShellArg tokenPath} ] || exit 0

    # git feeds the request as `key=value` lines on stdin. The `|| [ -n "$key" ]`
    # guard processes a final line that lacks a trailing newline.
    proto= host=
    while IFS='=' read -r key value || [ -n "$key" ]; do
      case "$key" in
        protocol) proto=$value ;;
        host) host=$value ;;
      esac
    done
    [ "$proto" = https ] && [ "$host" = github.com ] || exit 0

    token=$(<${lib.escapeShellArg tokenPath})
    [ -n "$token" ] || exit 0
    printf 'username=x-access-token\n'
    printf 'password=%s\n' "$token"
  '';

  # Secret-independent health probe: assert THIS example's wiring, not a
  # returned token. Credential helpers are additive across scopes, so `--get`
  # would return whatever helper has highest priority (a user's global config
  # could shadow it); `--get-all` plus an exact match is the honest check that
  # the system config registered our helper for github.com. Then run that exact
  # helper and require exit 0 with no output for the empty request fed here
  # (which never matches the host guard, so no token is read even once one is
  # delivered; the stdout redirect is belt-and-suspenders against a token
  # reaching the health-check log). Passes in CI and on a fresh boot with no
  # token.
  #
  # Kept as raw bash: the `git config --get-all | grep -qxF` probe relies on
  # `grep -q` exiting early, which under the bash writer's `set -o pipefail`
  # would surface git's SIGPIPE as a failure on a successful match. Pairs with
  # the credential helper above.
  # astlog-ignore: no-write-shell-script
  credentialHelperCheck = pkgs.writeShellScript "check-github-credential-helper" ''
    set -eu
    ${lib.getExe pkgs.git} config --get-all 'credential.https://github.com.helper' \
      | ${lib.getExe pkgs.gnugrep} -qxF ${credentialHelper}
    test -x ${credentialHelper}
    ${credentialHelper} get </dev/null >/dev/null
  '';
in {
  # System git config in `/etc/gitconfig`. Credential helpers are additive
  # across scopes, so a user's `~/.config/git/config` can add its own helper but
  # does not replace this one; no user here defines a github helper, so this is
  # the one that answers. git execs the helper directly because the value is an
  # absolute path.
  environment.etc."gitconfig".text = ''
    [credential "https://github.com"]
    	helper = ${credentialHelper}

    # Route SSH-style remotes through HTTPS so the same token authenticates
    # `git@github.com:` and `ssh://git@github.com/` clones. This applies to
    # every user on the node. Drop this block if a node should keep using SSH
    # keys for GitHub instead.
    [url "https://github.com/"]
    	insteadOf = git@github.com:
    	insteadOf = ssh://git@github.com/
  '';

  # `gh` does not use git's credential helper; it reads `GH_TOKEN` (or
  # `GITHUB_TOKEN`). It is left out of the global environment on purpose: an
  # exported token is visible in that process's `/proc/<pid>/environ`, is
  # inherited by every descendant, and can land in a core dump. Operators who
  # want the `gh` CLI authenticated point it at the same file per shell, e.g.
  #   export GH_TOKEN="$(cat /run/secrets/github/token)"
  # See the README for why this is not baked in.

  ix.healthChecks.github-credential-helper = {
    description = "git is wired to the synced-token credential helper";
    command = ["${credentialHelperCheck}"];
  };
}
