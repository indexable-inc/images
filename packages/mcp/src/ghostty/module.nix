{
  bundledSource,
  bundledTestPython,
  importTest,
  lib,
  pkgs,
}: let
  # Drive the Ghostty terminal over its AppleScript dictionary (Ghostty 1.3.2+):
  # `import ghostty`, then `await ghostty.surfaces()` reads every open surface
  # (id/tty/pid/cwd/name) into polars and `await ghostty.close_me()` closes the
  # window this session runs in. Pure Python shelling `osascript` on the loop; no
  # native binding, so a plain toPythonModule like `worktree`/`tasks`. macOS-only
  # at import; bundled only in `darwinExtraPackages`.
  ghosttyPythonSource = bundledSource {
    name = "ix-mcp-ghostty-python-source";
    path = ./.;
  };
  ghosttyModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-ghostty-python-module"
    {
      strictDeps = true;
      meta.description = "Ghostty AppleScript control bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/ghostty"
      mkdir -p "$site"
      cp -r ${ghosttyPythonSource}/ghostty/. "$site/"
    ''
  );
  ghosttyBundled = importTest [ghosttyModule] "ghostty" "import ghostty; print('ghostty-ok', all(callable(getattr(ghostty, n)) for n in ('surfaces', 'my_tty', 'my_surface', 'close', 'close_me', 'focus', 'activate', 'is_running')), ghostty.__version__)";
  # The ghostty module: pure-logic checks that need no Ghostty, osascript, or
  # display (the nix sandbox has none). Exercises the AppleScript-escape guard
  # (the injection fix), the ps-ancestry parse/walk, and the surfaces readout
  # split, and confirms the public coroutine surface.
  ghosttyTestPy = pkgs.writeText "ix-mcp-ghostty-test.py" ''
    # python
    import inspect

    import ghostty

    # Public surface is callable and async.
    for name in (
        "surfaces", "my_tty", "my_surface", "close", "close_me",
        "focus", "activate", "is_running",
    ):
        assert inspect.iscoroutinefunction(getattr(ghostty, name)), name

    # _escape_applescript neutralises a quote-injection payload, rejects newlines.
    assert ghostty._escape_applescript('a"b') == 'a\\"b'
    assert ghostty._escape_applescript("c\\d") == "c\\\\d"
    try:
        ghostty._escape_applescript("a\nb")
    except ghostty.GhosttyError:
        pass
    else:
        raise SystemExit("escape did not reject a newline")

    # _selector escapes the value: no raw quote reaches the whose-clause, so the
    # `" or true or "` predicate-injection cannot match every surface.
    sel = ghostty._selector(tty=None, id='" or true or "')
    assert sel.count('\\"') == 2, sel
    assert ghostty._selector(tty="/dev/ttys001", id=None) == (
        '(first terminal whose tty is "/dev/ttys001")'
    )
    # Fails closed unless exactly one selector is given (both set or both unset).
    for bad in ({}, {"tty": "/dev/ttys001", "id": "X"}):
        try:
            ghostty._selector(tty=bad.get("tty"), id=bad.get("id"))
        except ghostty.GhosttyError:
            pass
        else:
            raise SystemExit(f"_selector accepted ambiguous args: {bad}")

    # _parse_ps + _walk_to_tty resolve the controlling tty from a synthetic table.
    tree = ghostty._parse_ps("100 1 ??\n200 100 ttys003\n300 200 ??\n")
    assert tree[200] == (100, "ttys003"), tree
    assert ghostty._walk_to_tty(tree, 300) == "/dev/ttys003"
    assert ghostty._walk_to_tty(tree, 100) is None

    # _parse_surfaces splits the RS/FS readout and coerces pid to int.
    raw = "ID1\x1f/dev/ttys001\x1f42\x1f/tmp\x1fTitle\x1e"
    assert ghostty._parse_surfaces(raw) == [
        {"id": "ID1", "tty": "/dev/ttys001", "pid": 42,
         "working_directory": "/tmp", "name": "Title"}
    ]
    assert set(ghostty._surface_schema()) == set(ghostty._FIELDS)

    print("ghostty-ok")
  '';
  ghosttyTestPython = bundledTestPython [ghosttyModule];
  ghosttySmoke =
    pkgs.runCommand "ix-mcp-ghostty-smoke"
    {
      nativeBuildInputs = [ghosttyTestPython];
      strictDeps = true;
    }
    ''
      ${lib.getExe ghosttyTestPython} ${ghosttyTestPy} >stdout 2>stderr || {
        echo "ix-mcp ghostty smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      grep -qx 'ghostty-ok' stdout || {
        echo "ix-mcp ghostty smoke did not confirm the ghostty module:" >&2
        cat stdout stderr >&2
        exit 1
      }
      mkdir -p "$out"
    '';
in {
  module = ghosttyModule;
  darwinOnly = true;
  tests = lib.optionalAttrs pkgs.stdenv.hostPlatform.isDarwin {
    inherit
      ghosttyBundled
      ghosttySmoke
      ;
  };
}
