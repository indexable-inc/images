{
  bundledSource,
  importTest,
  lib,
  pkgs,
  privateSessionModule,
  testsRoot,
}: let
  # `google_auth`: Gmail + Calendar for the kernel, with self-service sign-in.
  # Pure Python (no cdylib): it shells to the bundled `gcal` binary
  # (`IX_GCAL_BIN`, set on the wrapper below) to sign in (`login()` drives
  # `gcal auth --json` and opens a browser), to sign out (`logout()`), and to
  # mint short-lived access tokens from the shared grant, which it wraps as a
  # `google.oauth2.credentials` object the official client accepts. The refresh
  # token / client secret stay inside `gcal`; only access tokens cross into
  # Python.
  googleAuthPythonSource = bundledSource {
    name = "ix-mcp-google-auth-python-source";
    path = ./.;
  };
  googleAuthModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-google-auth-python-module"
    {
      strictDeps = true;
      passthru.ixFirstPartyDeps = [privateSessionModule];
      meta.description = "Google OAuth credentials helper bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/google_auth"
      mkdir -p "$site"
      cp -r ${googleAuthPythonSource}/google_auth/. "$site/"
    ''
  );
  # The `google_auth` helper imports (pulling in google-auth) and exposes its
  # builders. A real token mint needs IX_GCAL_BIN + a prior `gcal auth`, so the
  # sandbox-safe assertion is the unset path: it must raise a clear, typed error
  # naming the missing piece rather than hanging or crashing vaguely.
  googleAuthBundled = importTest [googleAuthModule] "google-auth" ''
    import os

    import google_auth

    assert callable(google_auth.credentials)
    assert callable(google_auth.gmail) and callable(google_auth.calendar)
    # Self-service sign-in surface: login() is awaitable, status()/logout() are
    # plain calls. These are what makes Gmail discoverable and usable with no
    # host-side setup file.
    import asyncio as _asyncio

    assert _asyncio.iscoroutinefunction(google_auth.login)
    assert callable(google_auth.status) and callable(google_auth.logout)
    # The mail sender (issue #2523): awaitable, so it never blocks the loop.
    assert _asyncio.iscoroutinefunction(google_auth.send)

    # In a shared (multiplayer) room (IX_MCP_SHARED set) Gmail/Calendar are
    # refused before minting ever looks for the grant, so a personal mailbox
    # never reaches state other participants can see.
    os.environ["IX_MCP_SHARED"] = "1"
    os.environ["IX_GCAL_BIN"] = "/nonexistent/gcal"
    try:
        google_auth.credentials()
    except google_auth.GoogleAuthError as exc:
        assert "shared" in str(exc).lower(), exc
    else:
        raise SystemExit("expected GoogleAuthError in a shared room")

    # Incognito is the default: with IX_MCP_SHARED unset the gate passes, so
    # minting then fails on the missing binary instead -- proving the shared
    # gate is the only thing that blocked it above.
    os.environ.pop("IX_MCP_SHARED", None)
    os.environ.pop("IX_GCAL_BIN", None)
    try:
        google_auth.credentials()
    except google_auth.GoogleAuthError as exc:
        assert "IX_GCAL_BIN" in str(exc), exc
    else:
        raise SystemExit("expected GoogleAuthError when IX_GCAL_BIN is unset")

    # status() answers instead of raising: a not-signed-in session reports
    # signed_in=False so a caller can branch on it and offer login().
    state = google_auth.status()
    assert state["signed_in"] is False, state
    print("google-auth-ok")
  '';
  # Network-free unit tests for the `google_auth` mail sender (issue #2523):
  # MIME assembly, reply threading (threadId + In-Reply-To/References), and the
  # delivered-body readback, driven against a stub Gmail Resource. Only the
  # module's import-time deps are needed: googleapiclient itself is stubbed.
  googleAuthTestPython = pkgs.python3.withPackages (ps: [
    ps.pytest
    ps.pydantic
    ps.google-auth
    privateSessionModule
    googleAuthModule
  ]);
  googleAuthTestSource = builtins.path {
    name = "ix-mcp-google-auth-send-test";
    path = testsRoot + "/test_google_auth_send.py";
  };
  googleAuthTests =
    pkgs.runCommand "ix-mcp-google-auth-tests"
    {
      nativeBuildInputs = [googleAuthTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      cp ${googleAuthTestSource} "$TMPDIR/test_google_auth_send.py"
      ${lib.getExe googleAuthTestPython} -m pytest "$TMPDIR/test_google_auth_send.py" -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp google_auth tests failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';
in {
  module = googleAuthModule;
  tests = {
    inherit
      googleAuthBundled
      googleAuthTests
      ;
  };
}
