{
  lib,
  pkgs,
  secretRefs,
  ...
}:
let
  # The fleet declares this once; every node resolves it to the same runtime
  # path. Only the path is known at eval time. The token bytes live in the
  # 0600 file the ix secrets manager writes at runtime.
  tokenPath = secretRefs."github/token";

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

  # Secret-independent health probe: assert the wiring, not a returned token.
  # git must resolve the helper path, the helper must be executable, and it must
  # exit 0 while emitting nothing for the empty request fed here (which never
  # matches the host guard, so no token is read even once one is delivered). The
  # stdout redirect is belt-and-suspenders so a token can never reach the
  # health-check log. This passes in CI and on a fresh boot with no token.
  credentialHelperCheck = pkgs.writeShellScript "check-github-credential-helper" ''
    set -eu
    helper=$(${lib.getExe pkgs.git} config --get 'credential.https://github.com.helper')
    test -x "$helper"
    "$helper" get </dev/null >/dev/null
  '';
in
{
  # System git config. It sits below any user's `~/.config/git/config`, so an
  # operator can still override per user, but no user here defines a github
  # helper, so this is the one that answers. git execs the helper directly
  # because the value is an absolute path.
  environment.etc."gitconfig".text = ''
    [credential "https://github.com"]
    	helper = ${credentialHelper}

    # Route SSH-style remotes through HTTPS so the same token authenticates
    # `git@github.com:` and `ssh://git@github.com/` clones. Drop this block if
    # a node should keep using SSH keys for GitHub instead.
    [url "https://github.com/"]
    	insteadOf = git@github.com:
    	insteadOf = ssh://git@github.com/
  '';

  # `gh` does not use git's credential helper; it reads `GH_TOKEN`. It is left
  # out of the global environment on purpose (an exported token is visible in
  # every process's `/proc/<pid>/environ`). Operators who want the `gh` CLI
  # authenticated point it at the same file per shell, e.g.
  #   export GH_TOKEN="$(cat /run/secrets/github/token)"
  # See the README for why this is not baked in.

  ix.healthChecks.github-credential-helper = {
    description = "git is wired to the synced-token credential helper";
    command = [ "${credentialHelperCheck}" ];
  };
}
