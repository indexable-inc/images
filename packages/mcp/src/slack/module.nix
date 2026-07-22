{
  bundledSource,
  importTest,
  ixNotebookMcpModule,
  lib,
  pkgs,
  privateSessionModule,
  testsRoot,
}: let
  # Slack: read channels, messages, threads; send messages; search -- all per-user
  # with a self-service token flow. Pure Python over stdlib urllib + polars.
  # Per-user credential: SLACK_USER_TOKEN/SLACK_TOKEN env or ~/.config/slack/token
  # (mode 0600, written by slack.login(token)). Incognito sessions only (personal
  # workspace data never reaches a shared room). Cross-platform.
  slackPythonSource = bundledSource {
    name = "ix-mcp-slack-python-source";
    path = ./.;
  };
  slackModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-slack-python-module"
    {
      strictDeps = true;
      passthru.ixFirstPartyDeps = [
        ixNotebookMcpModule
        privateSessionModule
      ];
      meta.description = "Per-user Slack channels/messages/search bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/slack"
      mkdir -p "$site"
      cp -r ${slackPythonSource}/slack/. "$site/"
    ''
  );
  # The `slack` helper imports and exposes its public surface. A real API call
  # needs SLACK_USER_TOKEN + network, so the sandbox-safe assertions are: the
  # module imports, the public callables exist, an unconfigured session raises
  # SlackError with a helpful message, and IX_MCP_SHARED=1 refuses access.
  slackBundled = importTest [slackModule] "slack" ''
    import os

    import slack

    assert callable(slack.login) and callable(slack.logout) and callable(slack.status)
    import asyncio as _asyncio

    assert _asyncio.iscoroutinefunction(slack.channels)
    assert _asyncio.iscoroutinefunction(slack.dms)
    assert _asyncio.iscoroutinefunction(slack.messages)
    assert _asyncio.iscoroutinefunction(slack.thread)
    assert _asyncio.iscoroutinefunction(slack.send)
    assert _asyncio.iscoroutinefunction(slack.search)

    # In a shared (multiplayer) room Slack is refused before any network call,
    # so personal workspace data never reaches state other participants can see.
    os.environ["IX_MCP_SHARED"] = "1"
    try:
        _asyncio.run(slack.channels())
    except slack.SlackError as exc:
        assert "shared" in str(exc).lower(), exc
    else:
        raise SystemExit("expected SlackError in a shared room")

    # Incognito is the default: with IX_MCP_SHARED unset the shared guard
    # passes, so the next failure is a missing token -- proving the guard was
    # the only thing that blocked it above.
    os.environ.pop("IX_MCP_SHARED", None)
    # Ensure no token env vars or file are present.
    os.environ.pop("SLACK_USER_TOKEN", None)
    os.environ.pop("SLACK_TOKEN", None)
    try:
        _asyncio.run(slack.channels())
    except slack.SlackError as exc:
        assert "token" in str(exc).lower(), exc
    else:
        raise SystemExit("expected SlackError when no token is configured")

    # status() answers instead of raising when not configured.
    state = slack.status()
    assert state["configured"] is False, state
    print("slack-ok")
  '';
  # Network-free unit tests for the `slack` helper: module shape plus that
  # `send` builds the right chat.postMessage params for top-level vs. in-thread
  # replies (stubbing the one network primitive).
  slackTestPython = pkgs.python3.withPackages (ps: [
    ps.pytest
    ps.polars
    ps.pydantic
    privateSessionModule
    slackModule
  ]);
  slackTestSource = builtins.path {
    name = "ix-mcp-slack-test";
    path = testsRoot + "/test_slack.py";
  };
  typeHintSupport = builtins.path {
    name = "ix-mcp-slack-type-hint-support";
    path = testsRoot + "/type_hint_support.py";
  };
  slackTests =
    pkgs.runCommand "ix-mcp-slack-tests"
    {
      nativeBuildInputs = [slackTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      cp ${slackTestSource} "$TMPDIR/test_slack.py"
      cp ${typeHintSupport} "$TMPDIR/type_hint_support.py"
      ${lib.getExe slackTestPython} -m pytest "$TMPDIR/test_slack.py" -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp slack tests failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';
in {
  module = slackModule;
  tests = {
    inherit
      slackBundled
      slackTests
      ;
  };
}
