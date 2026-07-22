{
  bundledSource,
  importTest,
  pkgs,
  privateSessionModule,
}: let
  # Beeper: read chats and messages across every connected network, search, and
  # send -- a polars-shaped wrapper over the local Beeper Desktop HTTP API
  # (default http://localhost:23373). Pure Python over the bundled httpx + polars.
  # Per-user credential: BEEPER_ACCESS_TOKEN env or ~/.config/beeper/token
  # (mode 0600, written by beeper.login(token)). Incognito sessions only (personal
  # chats never reach a shared room). Cross-platform.
  beeperPythonSource = bundledSource {
    name = "ix-mcp-beeper-python-source";
    path = ./.;
  };
  beeperModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-beeper-python-module"
    {
      strictDeps = true;
      passthru.ixFirstPartyDeps = [privateSessionModule];
      meta.description = "Per-user Beeper Desktop chats/messages/search/send bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/beeper"
      mkdir -p "$site"
      cp -r ${beeperPythonSource}/beeper/. "$site/"
    ''
  );
  # The `beeper` helper imports and exposes its public surface. A real API call
  # needs BEEPER_ACCESS_TOKEN + a running Beeper Desktop, so the sandbox-safe
  # assertions are: the module imports, the public callables exist, an
  # unconfigured session raises BeeperError naming the token, and IX_MCP_SHARED=1
  # refuses access.
  beeperBundled = importTest [beeperModule] "beeper" ''
    import os

    import beeper

    assert callable(beeper.login) and callable(beeper.logout)
    import asyncio as _asyncio

    assert _asyncio.iscoroutinefunction(beeper.status)
    assert _asyncio.iscoroutinefunction(beeper.accounts)
    assert _asyncio.iscoroutinefunction(beeper.chats)
    assert _asyncio.iscoroutinefunction(beeper.messages)
    assert _asyncio.iscoroutinefunction(beeper.search)
    assert _asyncio.iscoroutinefunction(beeper.search_chats)
    assert _asyncio.iscoroutinefunction(beeper.send)

    # In a shared (multiplayer) room Beeper is refused before any network call,
    # so personal chats never reach state other participants can see.
    os.environ["IX_MCP_SHARED"] = "1"
    try:
        _asyncio.run(beeper.accounts())
    except beeper.BeeperError as exc:
        assert "shared" in str(exc).lower(), exc
    else:
        raise SystemExit("expected BeeperError in a shared room")

    # Incognito is the default: with IX_MCP_SHARED unset the shared guard
    # passes, so the next failure is a missing token -- proving the guard was
    # the only thing that blocked it above.
    os.environ.pop("IX_MCP_SHARED", None)
    os.environ.pop("BEEPER_ACCESS_TOKEN", None)
    try:
        _asyncio.run(beeper.accounts())
    except beeper.BeeperError as exc:
        assert "token" in str(exc).lower(), exc
    else:
        raise SystemExit("expected BeeperError when no token is configured")

    # Regression: a datetime column whose every value is empty/missing must not
    # raise (polars format inference has no sample) -- _frame emits a typed null
    # column instead. A mixed column parses the real value and nulls the empty.
    allempty = beeper._frame([{"timestamp": ""}], {"timestamp": beeper._TS})
    assert allempty["timestamp"].dtype == beeper._TS, allempty.schema
    assert allempty["timestamp"].to_list() == [None], allempty
    mixed = beeper._frame(
        [{"timestamp": "2026-01-01T00:00:00Z"}, {"timestamp": ""}],
        {"timestamp": beeper._TS},
    )
    assert mixed["timestamp"].dtype == beeper._TS, mixed.schema
    assert mixed["timestamp"].null_count() == 1, mixed

    print("beeper-ok")
  '';
in {
  module = beeperModule;
  tests = {
    inherit
      beeperBundled
      ;
  };
}
