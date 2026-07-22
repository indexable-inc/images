{
  bundledSource,
  bundledTestPython,
  ixNotebookMcpModule,
  lib,
  pkgs,
}: let
  # `nix`: parse a `nix --log-format internal-json` stream into polars frames (a
  # durable event log + a folded build-DAG view) and a live, self-closing
  # dashboard Resource. Pure Python over the bundled polars (+ the runtime's
  # register_resource when in a kernel), cross-platform, so every session can
  # `import nix` and `await nix.build(".#foo")`.
  nixPythonSource = bundledSource {
    name = "ix-mcp-nix-python-source";
    path = ./.;
  };
  nixModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-nix-python-module"
    {
      strictDeps = true;
      passthru.ixFirstPartyDeps = [ixNotebookMcpModule];
      meta.description = "nix internal-json -> polars + live build-DAG, bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/nix"
      mkdir -p "$site"
      cp -r ${nixPythonSource}/nix/. "$site/"
    ''
  );
  # The `nix` module: parse a captured internal-json stream (no subprocess, no
  # network, so the sandbox runs it) and assert the durable event log, the folded
  # build-DAG view, the live error capture, and the rendered tree. The fixture is
  # a real-shaped slice of a `nix build` stream: an eval activity, a queryPathInfo
  # with a child fileTransfer carrying progress, a build with a log line and a set
  # phase, the matching stops, and a trailing error msg.
  nixTestPy = pkgs.writeText "ix-mcp-nix-test.py" ''
    # python
    import polars as pl

    import nix

    stream = "\n".join(
        [
            '@nix {"action":"start","id":1,"level":4,"parent":0,"text":"evaluating","type":0}',
            '@nix {"action":"start","id":2,"level":4,"parent":0,"text":"querying","type":109,"fields":["/nix/store/x","https://cache"]}',
            '@nix {"action":"start","id":3,"level":4,"parent":2,"text":"downloading","type":101,"fields":["https://cache/x.narinfo"]}',
            '@nix {"action":"result","id":3,"type":105,"fields":[2,3,0,0]}',
            '@nix {"action":"stop","id":3}',
            '@nix {"action":"stop","id":2}',
            '@nix {"action":"start","id":4,"level":3,"parent":0,"text":"building","type":105,"fields":["/nix/store/x.drv","",1,1]}',
            '@nix {"action":"result","id":4,"type":104,"fields":["unpackPhase"]}',
            '@nix {"action":"result","id":4,"type":101,"fields":["compiling thing"]}',
            "not an @nix line, ignored",
            '@nix {"action":"stop","id":4}',
            # Real nix escapes ANSI as \u001b (valid JSON); the parser strips it.
            '@nix {"action":"msg","level":0,"msg":"error: \u001b[31mboom\u001b[0m","raw_msg":"error: boom"}',
        ]
    )

    log = nix.parse(stream)

    # Durable event log: one row per @nix line (the plain line is skipped), typed.
    ev = log.events
    assert isinstance(ev, pl.DataFrame), type(ev)
    assert ev.height == 11, ev.height
    assert ev.filter(pl.col("action") == "start").height == 4, ev
    # kind decodes both activity and result enums; ANSI is stripped from msg.
    assert set(ev["kind"].drop_nulls().to_list()) >= {"build", "fileTransfer", "progress"}, ev["kind"]

    # Folded DAG view: one row per activity, with the parent edge and depth.
    acts = log.activities
    assert acts.height == 4, acts
    ft = acts.filter(pl.col("kind") == "fileTransfer").row(0, named=True)
    assert ft["parent"] == 2 and ft["depth"] == 1, ft
    assert ft["done"] == 2 and ft["expected"] == 3, ft  # progress folded in

    bld = acts.filter(pl.col("kind") == "build").row(0, named=True)
    assert bld["status"] == "done", bld          # stop folded in
    assert bld["phase"] == "unpackPhase", bld    # setPhase folded in
    assert bld["last_log"] == "compiling thing", bld  # build log line folded in
    assert bld["drv"] == "/nix/store/x.drv", bld

    # Error captured from the msg line, ANSI stripped.
    assert log.error == "error: boom", repr(log.error)

    # Tree + html render don't crash and reflect the build.
    assert "building" in log.tree()
    assert "<pre" in log.resource_html()

    # Empty parse yields well-typed empty frames, never crashes.
    empty = nix.parse("")
    assert empty.events.height == 0 and empty.activities.height == 0
    assert empty.events.schema["seq"] == pl.Int64

    # A warning (level 1) or an info line containing "error:" is NOT the failure.
    warn = nix.parse(
        '@nix {"action":"msg","level":1,"msg":"warning: deprecated"}\n'
        '@nix {"action":"msg","level":3,"msg":"note: see error: above"}'
    )
    assert warn.error is None, warn.error

    # Malformed parent cycle must not infinite-recurse the activities/render path.
    cyc = nix.parse(
        '@nix {"action":"start","id":1,"parent":2,"text":"a","type":0}\n'
        '@nix {"action":"start","id":2,"parent":1,"text":"b","type":0}'
    )
    assert cyc.activities.height == 2, cyc.activities
    assert "a" in cyc.tree()

    # Non-int progress fields must not crash the Int64 activities schema.
    bad = nix.parse(
        '@nix {"action":"start","id":1,"parent":0,"text":"x","type":101}\n'
        '@nix {"action":"result","id":1,"type":105,"fields":["nan","oops",0,0]}'
    )
    assert bad.activities.row(0, named=True)["done"] == 0, bad.activities

    # attrs(): the flake-show parser flattens systemed + plain outputs, filtered
    # to the requested system; an omitted (empty) system contributes no rows.
    show = {
        "packages": {
            "aarch64-darwin": {"mcp": {"type": "derivation", "description": "the mcp"}},
            "x86_64-linux": {},
        },
        "nixosConfigurations": {"host": {"type": "nixos-configuration"}},
    }
    rows = nix._flake_show_rows(show, "aarch64-darwin")
    by = {(r["kind"], r["attr"]): r for r in rows}
    assert by[("packages", "mcp")]["description"] == "the mcp", rows
    assert ("nixosConfigurations", "host") in by, rows
    assert all(r["attr"] for r in rows), rows
    assert isinstance(nix._current_system(), str) and "-" in nix._current_system()

    # eval(): the arg builder quotes nothing through a shell and substitutes the
    # {system} template, so `--apply` rides as its own argv element.
    assert nix._eval_args(".#mcp") == ["eval", ".#mcp", "--json", "--no-warn-dirty"]
    assert nix._eval_args(".#mcp", apply="builtins.attrNames") == [
        "eval", ".#mcp", "--json", "--no-warn-dirty", "--apply", "builtins.attrNames",
    ]
    assert nix._eval_args(".#x", raw=True)[2] == "--raw", nix._eval_args(".#x", raw=True)
    sysd = nix._current_system()
    assert nix._eval_args(".#checks.{system}.lint")[1] == f".#checks.{sysd}.lint"
    assert nix._eval_args(".#checks.{system}", system="x86_64-linux")[1] == ".#checks.x86_64-linux"

    print("nix-ok", nix.__version__)
  '';
  nixTestPython = bundledTestPython [nixModule];
  nixSmoke =
    pkgs.runCommand "ix-mcp-nix-smoke"
    {
      nativeBuildInputs = [nixTestPython];
      strictDeps = true;
    }
    ''
      ${lib.getExe nixTestPython} ${nixTestPy} >stdout 2>stderr || {
        echo "ix-mcp nix smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      grep -q '^nix-ok' stdout || {
        echo "ix-mcp nix smoke did not confirm the nix module:" >&2
        cat stdout stderr >&2
        exit 1
      }
      mkdir -p "$out"
    '';
in {
  module = nixModule;
  tests = {
    inherit
      nixSmoke
      ;
  };
}
