{
  browserModule,
  bundledSource,
  bundledTestPythonWith,
  fleetModule,
  fontsConf,
  fsearchModule,
  lib,
  nuPyModule,
  pkgs,
  playwrightBrowsers,
  serverTestPython,
  shModule,
  testsRoot,
  tuiModule,
  typecheckTestPython,
  viewModule,
  vmkitModule,
  weaveModule,
}: let
  # The single-tool MCP server itself, a pure-Python package installed into the
  # pinned interpreter so the `ix-mcp` entrypoint, the one shared kernel, and the
  # bundled modules all share one environment. No build step: plain Python over
  # ipykernel + jupyter-client + the bundled modules already in this interpreter.
  ixNotebookMcpSource = bundledSource {
    name = "ix-notebook-mcp-source";
    path = ./.;
  };
  ixNotebookMcpModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-notebook-mcp-module"
    {
      strictDeps = true;
      # store.py rides the durable weave spool (index#3419), so every env
      # that bundles this package needs the weave client alongside it.
      propagatedBuildInputs = [weaveModule];
      # The server's full first-party import surface (store.py's weave is the
      # only top-level one; the rest are lazy/guarded in runtime.py and the
      # ipython bootstrap), declared for the per-module test-env closure below.
      passthru.ixFirstPartyDeps = [
        fleetModule
        fsearchModule
        nuPyModule
        shModule
        tuiModule
        viewModule
        vmkitModule
        weaveModule
      ];
      meta.description = "The ix notebook-first MCP server package";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/ix_notebook_mcp"
      mkdir -p "$site"
      cp -r ${ixNotebookMcpSource}/. "$site/"
    ''
  );
  # Locks the bind-address default: with a working `tailscale status --json`
  # in PATH, `_tailscale_ip()` returns the first IPv4 from `Self.TailscaleIPs`
  # so the Jupyter Server is reachable from any tailnet peer; with no tailscale
  # binary, it returns None so the CLI falls through to loopback. The mock
  # binary lives in TMP/path so we control PATH exactly without touching the
  # real tailscale state. The mock is shell, not a subprocess of an actual
  # tailscale, so this test runs in the Nix sandbox.
  bindDefaultTest = pkgs.writeText "ix-mcp-bind-default-test.py" ''
    # python
    from unittest.mock import patch
    from ix_notebook_mcp import cli

    status = {
        "BackendState": "Running",
        "Self": {
            "TailscaleIPs": ["100.64.0.7", "fd7a::1"],
            "DNSName": "node.tail-x.ts.net.",
        }
    }

    # Happy path: tailscale is up. The helper picks the first IPv4 and strips
    # the trailing dot from the DNS name.
    with patch.object(cli, "_tailscale_status", return_value=status):
        assert cli._tailscale_ip() == "100.64.0.7", f"got {cli._tailscale_ip()!r}"
        assert cli._tailscale_dns_name() == "node.tail-x.ts.net", f"got {cli._tailscale_dns_name()!r}"

    # Tailscale installed but stopped (or needs login): it still reports its
    # assigned IPs, but they are not bound to any interface, so the helper must
    # treat them as unusable and fall back to loopback.
    for state in ("Stopped", "NeedsLogin", "NoState"):
        stopped = {**status, "BackendState": state}
        with patch.object(cli, "_tailscale_status", return_value=stopped):
            assert cli._tailscale_ip() is None, f"{state}: expected None, got {cli._tailscale_ip()!r}"

    # No tailscale: the helpers return None so the CLI falls back to loopback.
    # Stubbing the inner _tailscale_status is more robust than juggling PATH or
    # the absolute fallback paths the real helper probes (which exist on hydra
    # outside the sandbox, so a PATH-only test would still find them).
    with patch.object(cli, "_tailscale_status", return_value=None):
        assert cli._tailscale_ip() is None, "expected None when tailscale is unavailable"
        assert cli._tailscale_dns_name() is None, "expected None when tailscale is unavailable"

    # IPv6-only or empty IP list: still None (the bind expects IPv4).
    with patch.object(
        cli,
        "_tailscale_status",
        return_value={"BackendState": "Running", "Self": {"TailscaleIPs": ["fd7a::1"]}},
    ):
        assert cli._tailscale_ip() is None, "IPv6-only TailscaleIPs should yield None"

    # _bindable: loopback is bindable; a reserved/unassigned address is not, so
    # the CLI falls back to loopback instead of crashing the dashboard.
    free = cli._free_port()
    assert cli._bindable("127.0.0.1", free) is True, "loopback must be bindable"
    assert cli._bindable("240.0.0.1", free) is False, "reserved address must be unbindable"

    print("bind-default-ok")
  '';
  bindDefaultSmoke =
    pkgs.runCommand "ix-mcp-bind-default-smoke"
    {
      nativeBuildInputs = [serverTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      ${lib.getExe serverTestPython} ${bindDefaultTest} >stdout 2>stderr || {
        echo "ix-mcp bind-default smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      grep -qx 'bind-default-ok' stdout || {
        echo "ix-mcp bind-default smoke did not confirm helper behaviour:" >&2
        cat stdout stderr >&2
        exit 1
      }
      mkdir -p "$out"
    '';
  # Exercises _resolve_ssh_auth_sock: the helper must redirect SSH_AUTH_SOCK to
  # the 1Password agent on darwin when the Apple launchd socket (or no socket)
  # is present, must leave a custom non-Apple agent alone, and must always
  # return None on non-darwin platforms. Dependency-free pure Python, so the
  # build sandbox runs it.
  sshAuthSockTest = pkgs.writeText "ix-mcp-ssh-auth-sock-test.py" ''
    # python
    import os
    import tempfile
    from pathlib import Path

    from ix_notebook_mcp.cli import _resolve_ssh_auth_sock

    with tempfile.TemporaryDirectory() as tmp:
        home = Path(tmp)
        op_dir = home / "Library" / "Group Containers" / "2BUA8C4S2C.com.1password" / "t"
        op_dir.mkdir(parents=True)
        op_sock = op_dir / "agent.sock"
        op_sock.touch()

        # darwin + unset SSH_AUTH_SOCK -> forward to 1Password
        result = _resolve_ssh_auth_sock(None, home, "darwin", exists=os.path.exists)
        assert result == str(op_sock), f"expected op_sock, got {result!r}"

        # darwin + Apple launchd socket -> forward to 1Password
        apple = "/var/run/com.apple.launchd.XYZ123/Listeners"
        result = _resolve_ssh_auth_sock(apple, home, "darwin", exists=os.path.exists)
        assert result == str(op_sock), f"expected op_sock for apple agent, got {result!r}"

        # darwin + custom non-Apple agent -> do not override
        custom = "/run/user/1000/gnupg/S.gpg-agent.ssh"
        result = _resolve_ssh_auth_sock(custom, home, "darwin", exists=os.path.exists)
        assert result is None, f"must not clobber custom agent, got {result!r}"

        # non-darwin platform -> always None, even with op sock present
        for plat in ("linux", "win32"):
            result = _resolve_ssh_auth_sock(None, home, plat, exists=os.path.exists)
            assert result is None, f"expected None on {plat!r}, got {result!r}"
            result = _resolve_ssh_auth_sock(apple, home, plat, exists=os.path.exists)
            assert result is None, f"expected None on {plat!r} with apple sock, got {result!r}"

        # darwin but 1Password socket absent -> None (do not crash)
        missing_home = Path(tmp) / "missing"
        missing_home.mkdir()
        result = _resolve_ssh_auth_sock(None, missing_home, "darwin", exists=os.path.exists)
        assert result is None, f"expected None when op sock absent, got {result!r}"

    print("ssh-auth-sock-ok")
  '';
  sshAuthSockSmoke =
    pkgs.runCommand "ix-mcp-ssh-auth-sock-smoke"
    {
      nativeBuildInputs = [serverTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      ${lib.getExe serverTestPython} ${sshAuthSockTest} >stdout 2>stderr || {
        echo "ix-mcp ssh-auth-sock smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      grep -qx 'ssh-auth-sock-ok' stdout || {
        echo "ix-mcp ssh-auth-sock smoke did not confirm helper behaviour:" >&2
        cat stdout stderr >&2
        exit 1
      }
      mkdir -p "$out"
    '';
  # Exercises the in-kernel runtime (ix_notebook_mcp/runtime.py) in-process: two
  # jobs run concurrently on one event loop, neither blocks the other, each keeps
  # its own captured stdout, and the trailing expression is captured as the
  # result. This is the core "multiple async" contract, provable without a kernel
  # or network, so the sandbox runs it.
  runtimeTestPy = pkgs.writeText "ix-mcp-runtime-test.py" ''
    # python
    import asyncio

    from ix_notebook_mcp import runtime

    ns = {}
    runtime.install(ns)
    jobs = ns["jobs"]
    run = ns["__ix_run"]

    async def main():
        a = await run("import asyncio\nfor i in range(3):\n    print('A', i)\n    await asyncio.sleep(0.05)\nResult.text('A done')", budget=0.02, name="A")
        b = await run("import asyncio\nfor i in range(3):\n    print('B', i)\n    await asyncio.sleep(0.05)\nResult.text('B done')", budget=0.02, name="B")
        assert a.running() and b.running(), (a.status, b.status)
        assert len(jobs) == 2, len(jobs)
        await asyncio.sleep(0.5)
        assert a.status == "done" and b.status == "done", (a.status, b.status)
        assert "A 0" in a.output and "B 0" in b.output, (a.output, b.output)
        assert a.result.llm_result == "A done" and b.result.llm_result == "B done", (a.result, b.result)
        # paging ops over a finished job keep a large output recoverable
        assert "A 0" in a.head(10000) and a.slice(0, 1) == a.output[0]
        assert a.lines(0, 1).startswith("0: ")
        g = a.grep("A 1")
        assert "A 1" in g and g.split(":", 1)[0].strip().isdigit(), g
        assert "no lines match" in a.grep("nonesuch-xyz-pattern")
        # full sizes the server uses to detect a truncated reply
        s = runtime._job_summary(a)
        assert s["output_chars"] == len(a.output) and s["result_chars"] == len("A done"), s
        # elapsed_s rides every summary so the per-call reply reports the run's cost
        assert isinstance(s["elapsed_s"], float) and s["elapsed_s"] >= 0.0, s
        # history() indexes the runs and returns a Result naming both jobs
        h = ns["history"]()
        assert isinstance(h, runtime.Result) and a.id in h.llm_result and b.id in h.llm_result

        # A KeyboardInterrupt the user's own code raises keeps its real traceback
        # (it is NOT misattributed to the wedge watchdog, whose flag is unset here).
        k = await run("raise KeyboardInterrupt", budget=1.0, name="kbi")
        assert k.status == "error", k.status
        assert "Traceback" in k.error and "KeyboardInterrupt" in k.error, k.error
        assert "asyncio.to_thread" not in k.error, k.error

        # When the watchdog flag IS set (as the SIGUSR2 handler does before raising),
        # the same interrupt yields the actionable wedge message instead. The cell
        # sets the flag on its own running job via the runtime ContextVar.
        w = await run(
            "import ix_notebook_mcp.runtime as _rt\n"
            "_rt._ix_current.get().interrupted_by_watchdog = True\n"
            "raise KeyboardInterrupt",
            budget=1.0,
            name="kbi-watchdog",
        )
        assert w.status == "error", w.status
        assert "asyncio.to_thread" in w.error and "Traceback" not in w.error, w.error

        # A print-only cell (last statement is None) returns its captured stdout,
        # so what it printed reaches the model -- a notebook's behavior.
        p = await run("print('hello-from-stdout')", budget=1.0, name="printed")
        assert p.status == "done", (p.status, p.error)
        assert isinstance(p.result, runtime.Result), type(p.result)
        assert "hello-from-stdout" in p.result.llm_result, p.result.llm_result
        # A silent side-effecting cell returns a quiet confirmation.
        q = await run("x_side_effect = 1", budget=1.0, name="silent")
        assert q.status == "done", (q.status, q.error)
        assert "done" in q.result.llm_result, q.result.llm_result

        # A bare final value that already renders richly is auto-wrapped in
        # Result.of, so `df` on the last line just works without an explicit Result.
        d = await run("import polars as pl\npl.DataFrame({'x': [1, 2]})", budget=2.0, name="auto-df")
        assert d.status == "done", (d.status, d.error)
        assert isinstance(d.result, runtime.Result), type(d.result)
        assert d.result.llm_result.startswith("shape: (2, 1) | x:Int64"), d.result.llm_result
        assert "[[x]; [1], [2]]" in d.result.llm_result, d.result.llm_result
        import json as _json
        import subprocess as _subprocess
        import tempfile as _tempfile
        from pathlib import Path as _Path

        _nuon_path = _Path(_tempfile.mkdtemp()) / "df.nuon"
        _nuon_path.write_text(d.result.llm_result.split("\n", 1)[1])
        _parsed = _json.loads(_subprocess.check_output(
            ["nu", "-c", f"open --raw {_nuon_path} | from nuon | to json -r"],
            text=True,
        ))
        assert _parsed == [{"x": 1}, {"x": 2}], _parsed

        class Split:
            def __ix_html__(self):
                return "<strong>human</strong>"
            def __ix_llm__(self):
                return {"answer": 42, "tags": ["nuon", "llm"]}

        split = runtime.Result.of(Split())
        assert split.user_html == "<strong>human</strong>", split.user_html
        _split_path = _Path(_tempfile.mkdtemp()) / "split.nuon"
        _split_path.write_text(split.llm_result)
        _split = _json.loads(_subprocess.check_output(
            ["nu", "-c", f"open --raw {_split_path} | from nuon | to json -r"],
            text=True,
        ))
        assert _split == {"answer": 42, "tags": ["nuon", "llm"]}, (split.llm_result, _split)
        from ix_notebook_mcp import outputs as _outputs_for_llm

        _bundle = split._repr_mimebundle_()
        assert runtime.IX_LLM_MIME in _bundle and _bundle["text/html"] == "<strong>human</strong>", _bundle
        _content = _outputs_for_llm.to_mcp([{"output_type": "display_data", "data": _bundle}])
        assert len(_content) == 1 and _content[0].text == split.llm_result, _content
        # Jupyter semantics: the last expression IS the result, whatever its type.
        sc = await run("1 + 1", budget=2.0, name="scalar")
        assert sc.status == "done", (sc.status, sc.error)
        assert "2" in sc.result.llm_result, sc.result.llm_result
        # ...and stdout printed along the way rides with a bare final value.
        both = await run("print('logged')\n40 + 2", budget=2.0, name="print-and-value")
        assert both.status == "done", (both.status, both.error)
        assert "logged" in both.result.llm_result and "42" in both.result.llm_result, (
            both.result.llm_result
        )

        # A cell ending in a failed process Output is loud on every surface a
        # watcher reads (issue #1766: a build dead on ENOSPC read as
        # still-compiling): the streamed stdout carries the failure line, so
        # paging a backgrounded job's .output/.tail() shows the terminal state,
        # and the result's model text leads AND ends with the exit marker. The
        # Output itself is falsy. (`sh` is retired; the runner is the private
        # `_exec` the kernel's own internals still use.)
        fsh = await run("from sh import _exec\nawait _exec('echo diag-line; exit 7')", budget=10.0, name="failed-exec")
        assert fsh.status == "done", (fsh.status, fsh.error)
        assert "diag-line" in fsh.output and "[exit 7]" in fsh.output, fsh.output
        assert fsh.result.llm_result.splitlines()[0].startswith("[exit 7]"), fsh.result.llm_result
        assert fsh.result.llm_result.rstrip().endswith("[exit 7]"), fsh.result.llm_result
        assert fsh.result.exit_code == 7 and not fsh.result.ok, fsh.result.exit_code
        assert bool(fsh.result) is False, "a failed Output must be falsy"

        # .result raises while the job runs (a misleading None would read as
        # "finished with no value"); .done()/.ok track the lifecycle.
        slow = await run("import asyncio\nawait asyncio.sleep(0.4)\nResult.text('late')", budget=0.02, name="slow")
        assert slow.running() and not slow.done(), slow.status
        try:
            _ = slow.result
            raise AssertionError("expected .result to raise while running")
        except runtime.JobStillRunning:
            pass
        await slow.task
        assert slow.done() and slow.ok, slow.status
        assert slow.result.llm_result == "late", slow.result

        # Job.wait: a timed wait that never raises -- one cell replaces a
        # sleep-and-poll loop. At a short deadline the job is still running;
        # with no deadline it returns the finished job.
        slow2 = await run("import asyncio\nawait asyncio.sleep(0.3)\nResult.text('w')", budget=0.02, name="wait")
        assert (await slow2.wait(0.01)).running(), slow2.status
        assert (await slow2.wait()).done() and slow2.result.llm_result == "w"

        # A Result nested inside a Result (llm_result=Result.text(...)) is
        # flattened to its model text at construction, so the summary/paging
        # path never hits a non-str ("Result object is not subscriptable").
        nested = runtime.Result(user_html="<b>x</b>", llm_result=runtime.Result.text("inner"))
        assert nested.llm_result == "inner", nested.llm_result
        nj = await run(
            "Result(user_html='<b>x</b>', llm_result=Result.text('inner-e2e'))",
            budget=2.0, name="nested",
        )
        assert nj.status == "done", (nj.status, nj.error)
        assert "inner-e2e" in nj.tail(100), nj.tail(100)
        assert runtime._job_summary(nj)["result_chars"] == len("inner-e2e")
        # Any other non-str llm_result coerces to its repr rather than crash later.
        odd = runtime.Result(user_html="x", llm_result=123)
        assert odd.llm_result == "123", odd.llm_result

    asyncio.run(main())
    # api(): a discoverable catalog of kernel builtins + bundled modules. `nu`
    # is the catalogued shell-out path; the retired `sh` is NOT listed (though it
    # stays bound as a disabled shim so a stale call fails loudly, tested below).
    cat = ns["api"]()
    names = set(cat["name"].to_list())
    assert {"Result", "cells", "jobs", "nu", "api"} <= names, names
    assert "sh" not in names and "zsh" not in names, names
    filt = cat.filter(cat["name"] == "cells")
    assert filt.height == 1, filt

    # grep/find/spotlight (the fsearch search helpers) and view are pre-bound in
    # the namespace (no import needed), the way Result/cells/jobs are, so
    # `await grep(...)` / `view.tree(...)` just work.
    assert callable(ns.get("grep")) and callable(ns.get("find")), (ns.get("grep"), ns.get("find"))
    assert callable(ns.get("spotlight")), ns.get("spotlight")

    # `sh`/`zsh` stay bound but are DISABLED: calling either raises a migration
    # hint pointing at `await nu(...)`, so an old transcript fails loudly rather
    # than with a bare NameError.
    async def _sh_disabled() -> None:
        for expr in ("await sh('echo hi')", "await zsh('echo hi')", "await sh(['echo', 'hi'])"):
            r = await run(expr, budget=2.0, name="sh-disabled")
            assert r.status == "error", (expr, r.status)
            assert "await nu" in (r.error or ""), (expr, r.error)

    asyncio.run(_sh_disabled())
    assert callable(getattr(ns.get("view"), "tree", None)), ns.get("view")

    # Result.llm_images downscale a large raster to <= _IMAGE_MAX_DIM on its
    # longest edge before base64-encoding it for the model (Pillow is present via
    # matplotlib), so a full-page screenshot does not cost vision tokens at full
    # resolution.
    import base64 as _b64
    import io as _io

    from PIL import Image as _Image

    _buf = _io.BytesIO()
    _Image.new("RGB", (3000, 1500), (10, 20, 30)).save(_buf, format="PNG")
    _coerced = runtime._coerce_image(_buf.getvalue())
    assert _coerced is not None, _coerced
    _w, _h = _Image.open(_io.BytesIO(_b64.b64decode(_coerced["data"]))).size
    assert max(_w, _h) <= runtime._IMAGE_MAX_DIM, (_w, _h, runtime._IMAGE_MAX_DIM)

    # The dimension cap alone does not bound bytes: a busy 1280px screenshot stays
    # megabytes as PNG. So _fit_image_bytes also enforces _IMAGE_MAX_BYTES, falling
    # back to JPEG (and further downscales) -- a high-entropy image comes back well
    # under the byte cap instead of flooding the model's reply with base64.
    import os as _osr

    _noisy = _Image.frombytes("RGB", (3000, 1500), _osr.urandom(3000 * 1500 * 3))
    _nbuf = _io.BytesIO()
    _noisy.save(_nbuf, format="PNG")
    assert len(_nbuf.getvalue()) > runtime._IMAGE_MAX_BYTES, len(_nbuf.getvalue())
    _fit = runtime._coerce_image(_nbuf.getvalue())
    _raw = _b64.b64decode(_fit["data"])
    assert len(_raw) <= runtime._IMAGE_MAX_BYTES, ("over byte cap", len(_raw))
    _fw, _fh = _Image.open(_io.BytesIO(_raw)).size
    assert max(_fw, _fh) <= runtime._IMAGE_MAX_DIM, (_fw, _fh)
    # A small lossless image fits both caps and is kept byte-for-byte (a crisp PNG
    # for UI/diagrams is never needlessly re-encoded).
    _sbuf = _io.BytesIO()
    _Image.new("RGB", (200, 100), (10, 20, 30)).save(_sbuf, format="PNG")
    _small = _sbuf.getvalue()
    assert _b64.b64decode(runtime._coerce_image(_small)["data"]) == _small

    # outputs.text() renders an over-cap block as a head+tail preview (not a
    # one-sided clip) with paging guidance, and honours IX_MCP_MAX_RESULT_CHARS.
    import importlib as _il
    import os as _os

    from ix_notebook_mcp import outputs as _outputs

    _os.environ["IX_MCP_MAX_RESULT_CHARS"] = "1000"
    _il.reload(_outputs)
    _blk = _outputs.text("HEAD" + ("z" * 5000) + "TAIL").text
    assert _blk.startswith("HEAD") and _blk.endswith("TAIL"), _blk[:40]
    assert "output too large" in _blk and len(_blk) < 2000, len(_blk)
    _os.environ.pop("IX_MCP_MAX_RESULT_CHARS", None)
    _il.reload(_outputs)

    # outputs._image is the final byte net for every image reaching the model: a
    # small image becomes a real image block, but an oversize blob (e.g. a raw
    # display(fig) bundle that never went through the kernel's fitter) is dropped
    # with a short note rather than dumped as megabytes of base64.
    _ok = _outputs._image("image/png", _b64.b64encode(_small).decode("ascii"))
    assert _ok.type == "image", _ok
    _over = _b64.b64encode(b"x" * (_outputs.MAX_IMAGE_BYTES + 5000)).decode("ascii")
    _dropped = _outputs._image("image/png", _over)
    assert _dropped.type == "text" and "dropped" in _dropped.text, _dropped
    assert len(_dropped.text) < 400, len(_dropped.text)
    # End to end: an oversize image in a display bundle yields no giant text block.
    _rendered = _outputs.to_mcp([{"output_type": "display_data", "data": {"image/png": _over}}])
    assert all(_c.type == "text" for _c in _rendered), [_c.type for _c in _rendered]
    assert max(len(_c.text) for _c in _rendered) < 400, [len(_c.text) for _c in _rendered]

    # view.tree lists but does not descend into heavy dirs (node_modules, ...)
    # unless all=True, so a project's structure is not buried under vendored files.
    import pathlib as _pl
    import tempfile as _tf

    import view as _view

    _root = _tf.mkdtemp()
    _pl.Path(_root, "src").mkdir()
    _pkg = _pl.Path(_root, "node_modules", "pkg")
    _pkg.mkdir(parents=True)
    (_pkg / "index.js").write_text("x")
    _collapsed = _view.tree(_root, depth=3)
    _walked = _view.tree(_root, depth=3, all=True)
    assert _walked.height > _collapsed.height, (_collapsed.height, _walked.height)
    _names = _collapsed["name"].to_list()
    assert any("node_modules" in n for n in _names), _names
    assert not any("index.js" in n for n in _names), _names

    # .gitignore-aware pruning (git is on PATH in this sandbox): a dir the repo
    # ignores but that is NOT in the static denylist still collapses, and an
    # ignored file drops entirely.
    import shutil as _shutil
    import subprocess as _sub

    if _shutil.which("git"):
        _g = _tf.mkdtemp()
        _pl.Path(_g, "src").mkdir()
        _gen = _pl.Path(_g, "generated")
        _gen.mkdir()
        (_gen / "big.py").write_text("x")
        (_pl.Path(_g) / "debug.log").write_text("x")
        (_pl.Path(_g) / ".gitignore").write_text("generated/" + chr(10) + "*.log" + chr(10))
        _sub.run(["git", "init", "-q"], cwd=_g, check=True)
        _gi = _view.tree(_g, depth=3)["name"].to_list()
        assert any("generated" in n for n in _gi), _gi
        assert not any("big.py" in n for n in _gi), _gi
        assert not any("debug.log" in n for n in _gi), _gi
        assert any("src" in n for n in _gi), _gi

        # view.ls stays flat but flags git-ignored entries in an `ignored` column
        # (it never drops them, unlike tree): the *.log file is ignored, src is not.
        _lsg = _view.ls(_g)
        assert "ignored" in _lsg.columns, _lsg.columns
        _byname = {r["name"]: r["ignored"] for r in _lsg.iter_rows(named=True)}
        assert _byname.get("debug.log") is True, _byname
        assert _byname.get("src") is False, _byname

    print("runtime-ok")
  '';
  runtimeSmoke =
    pkgs.runCommand "ix-mcp-runtime-smoke"
    {
      # git is on PATH so the view.tree .gitignore-pruning assertion can init a
      # throwaway repo; without it that path falls back to the denylist (still
      # covered by the no-git case in the same test).
      nativeBuildInputs = [
        serverTestPython
        pkgs.git
        pkgs.nushell
      ];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      ${lib.getExe serverTestPython} ${runtimeTestPy} >stdout 2>stderr || {
        echo "ix-mcp runtime smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      grep -qx 'runtime-ok' stdout || {
        echo "ix-mcp runtime smoke did not confirm concurrent jobs:" >&2
        cat stdout stderr >&2
        exit 1
      }
      mkdir -p "$out"
    '';
  # The session-file contract: run cells against a session store, checkpoint,
  # "restart" into a fresh namespace, and reopen -- the checkpoint restores the
  # state instantly (including a function defined in a cell, which needs the
  # bundled dill), the one cell newer than the checkpoint replays, a row left
  # 'running' by the dead server is marked interrupted, and a second reopen has
  # nothing to replay (the restore folds everything into a fresh checkpoint).
  sessionTestPy = pkgs.writeText "ix-mcp-session-test.py" ''
    # python
    import asyncio
    import sys
    import tempfile

    import dill  # the checkpoint serializer must be bundled in this interpreter

    # Hermetic: the session contract runs over an in-memory Weave ABI double
    # (tests/weave_stub.py, copied next to this script by the derivation);
    # real-server fidelity is pinned by tests/test_weave_integration.py.
    sys.path.insert(0, ".")
    import weave_stub

    weave_stub.install()

    from ix_notebook_mcp import runtime, store

    path = tempfile.mktemp(suffix=".ixnb")

    def wire(conn, ns):
        runtime._store = store
        runtime._store_conn = conn
        runtime._user_ns = ns
        runtime._SESSION = True
        runtime._baseline_names = frozenset(ns)

    async def first_run():
        conn = store.connect(path)
        ns = {"Result": runtime.Result}
        wire(conn, ns)
        a = await runtime.__ix_run("x = 40\ndef double(n):\n    return n * 2\nResult.ok('a')")
        assert a.status == "done", (a.status, a.error)
        await runtime._snapshot_now()
        b = await runtime.__ix_run("y = double(x) + 4\nResult.ok('b')")
        assert b.status == "done", (b.status, b.error)
        # A row left 'running' by a server that died mid-cell.
        store.start(conn, id="dead", name="dead", code="zz", started_at=1.0)
        conn.close()

    asyncio.run(first_run())

    async def reopen():
        conn = store.connect(path)
        assert store.mark_interrupted(conn, ended_at=2.0) == 1
        assert store.get(conn, "dead")["status"] == "interrupted"
        ns = {"Result": runtime.Result}
        wire(conn, ns)
        runtime.jobs.clear()
        await runtime.__ix_restore()
        snap = store.latest_snapshot(conn)
        assert snap is not None, "restore must fold a fresh checkpoint"
        assert store.replayable(conn, since=snap["created_at"]) == [], "second reopen must replay nothing"
        conn.close()
        return ns

    ns = asyncio.run(reopen())
    assert ns["x"] == 40, ns.get("x")
    assert ns["double"](3) == 6
    assert ns["y"] == 84, ns.get("y")
    print("session-ok")
  '';
  sessionSmoke =
    pkgs.runCommand "ix-mcp-session-smoke"
    {
      nativeBuildInputs = [serverTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      cd "$TMPDIR"
      cp ${builtins.path {
        name = "ix-mcp-weave-stub";
        path = testsRoot + "/weave_stub.py";
      }} weave_stub.py
      ${lib.getExe serverTestPython} ${sessionTestPy} >stdout 2>stderr || {
        echo "ix-mcp session smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      grep -qx 'session-ok' stdout || {
        echo "ix-mcp session smoke did not confirm the reopen contract:" >&2
        cat stdout stderr >&2
        exit 1
      }
      mkdir -p "$out"
    '';
  # Boots a real kernel and proves the two signal-driven recoveries for a cell
  # that blocks the kernel's event loop with a synchronous call:
  #   1. kernel_trace (SIGUSR1 -> faulthandler) returns the kernel's stack WHILE
  #      the loop is wedged, since it never touches the execute channel.
  #   2. the wedge watchdog (SIGUSR2 -> KeyboardInterrupt) breaks the block past
  #      budget+grace, returns a 'wedged' summary in about budget+grace (not the
  #      sleep's full duration), and leaves the kernel usable for the next cell.
  # Guards the fix for the opaque "Timeout waiting for output" a forgotten
  # blocking call used to cause. SIGINT is NOT enough here: every cell is async
  # (await __ix_exec), and ipykernel interrupts async cells by cancelling the
  # asyncio task, which a synchronous call never yields to.
  wedgeTestPy = pkgs.writeText "ix-mcp-wedge-test.py" ''
    # python
    import asyncio
    import os
    import tempfile
    from pathlib import Path

    from ix_notebook_mcp import cli
    from ix_notebook_mcp.config import Config
    from ix_notebook_mcp.kernel import Kernel

    # Install the shipped IPython startup so the in-kernel runtime (__ix_exec,
    # Result, jobs, the SIGUSR1/SIGUSR2 handlers) loads in the booted kernel,
    # exactly as the CLI wires it.
    os.environ["IPYTHONDIR"] = str(cli._prepare_ipython_startup(0))
    config = Config(workdir=Path(tempfile.mkdtemp()), wedge_grace=1.0, max_budget=2.0)


    async def main():
        kernel = Kernel(config)
        await kernel.start()
        try:
            loop = asyncio.get_running_loop()

            # (1) A trace must come back even while a cell blocks the loop. Start a
            # blocking cell (budget high enough that the watchdog does not fire),
            # let it enter the sleep, then dump the kernel stack out of band.
            blocking = asyncio.ensure_future(
                kernel.python_exec("import time\ntime.sleep(6)\nResult.ok('slept')", budget=30.0, name="blk")
            )
            await asyncio.sleep(1.0)
            trace = await kernel.dump_trace()
            assert "Thread" in trace and 'File "' in trace, ("not a faulthandler dump", trace)
            _, blk = await blocking
            assert blk is not None and blk["status"] == "done", blk

            # (2) A cell that blocks past budget+grace is interrupted via SIGUSR2
            # and the kernel is usable for the next cell.
            started = loop.time()
            _, summary = await kernel.python_exec(
                "import time\ntime.sleep(30)\nResult.ok('done')", budget=0.5, name="block"
            )
            elapsed = loop.time() - started
            assert summary is not None and summary["status"] == "wedged", summary
            assert elapsed < 15, ("watchdog did not fire promptly", elapsed)
            assert "asyncio.to_thread" in summary["error"], summary
            # a wedged reply still carries elapsed_s (the slowest case the field
            # exists to surface), reporting the seconds the call blocked
            assert isinstance(summary["elapsed_s"], float) and summary["elapsed_s"] >= 0.5, summary

            _, after = await kernel.python_exec("Result.text('alive')", budget=10.0, name="after")
            assert after is not None and after["status"] == "done", after
            assert after["result"] is not None, after

            # (3) Cancelling an in-flight python_exec (the client cancels the call)
            # must not desync the shared shell channel. Start a cell that
            # backgrounds at its small budget, cancel the foreground wait while the
            # reply is in flight, then prove a later call still runs.
            inflight = asyncio.ensure_future(
                kernel.python_exec("await asyncio.sleep(5)\nResult.ok('slept')", budget=0.4, name="cancelme")
            )
            await asyncio.sleep(0.1)
            inflight.cancel()
            try:
                await inflight
            except asyncio.CancelledError:
                pass
            _, revived = await kernel.python_exec("Result.text('post-cancel')", budget=10.0, name="post-cancel")
            assert revived is not None and revived["status"] == "done", revived
            assert revived["result"] is not None, revived

            # (4) The python_exec TOOL clamps an oversized budget to max_budget so a
            # giant foreground wait cannot sit on the one shell channel: the call
            # returns within the cap (not the requested 600s) and says it clamped.
            from ix_notebook_mcp import tools
            from ix_notebook_mcp.config import set_config
            from ix_notebook_mcp.kernel import set_kernel
            from mcp.shared.exceptions import McpError

            set_config(config)
            set_kernel(kernel)

            try:
                await tools.python_exec("Result.ok('blocked')", budget=1.0, intent="blocked first")
            except McpError as exc:
                assert "session_set_name" in str(exc), exc
            else:
                raise AssertionError("python_exec ran before the session was named")

            named = await tools.session_set_name("wedge smoke")
            assert "wedge smoke" in " ".join(getattr(c, "text", "") or "" for c in named), named
            topic = await tools.topic_set("wedge validation")
            assert "wedge validation" in " ".join(getattr(c, "text", "") or "" for c in topic), topic

            started = loop.time()
            clamped = await tools.python_exec(
                "await asyncio.sleep(30)\nResult.ok('done')", budget=600.0, intent="bigbudget"
            )
            elapsed = loop.time() - started
            assert elapsed < 10, ("budget was not clamped", elapsed)
            # python_exec returns a CallToolResult (MCP Apps: the human view
            # rides its _meta); the model-facing blocks live on .content.
            note = " ".join(getattr(c, "text", "") or "" for c in clamped.content)
            assert "clamped" in note, note
        finally:
            await kernel.shutdown()


    asyncio.run(main())
    print("wedge-ok")
  '';
  wedgeSmoke =
    pkgs.runCommand "ix-mcp-wedge-smoke"
    {
      nativeBuildInputs = [serverTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      ${lib.getExe serverTestPython} ${wedgeTestPy} >stdout 2>stderr || {
        echo "ix-mcp wedge smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      grep -qx 'wedge-ok' stdout || {
        echo "ix-mcp wedge smoke did not confirm the watchdog:" >&2
        cat stdout stderr >&2
        exit 1
      }
      mkdir -p "$out"
    '';
  # An externally killed kernel must be reported as `kernel died (pid N, signal
  # S); respawning` -- never a generic 'wedged' timeout -- with kernel_trace
  # naming the gone process and the death watch respawning it eagerly, not on
  # the next execute (packages/mcp/tests/test_kernel_death.py, index#2339: a
  # broad pkill SIGTERM'd a session's kernel and every symptom read as a wedge).
  # Boots a real kernel, so it reuses the full interpreter plus pytest.
  kernelDeathTestSource = builtins.path {
    name = "ix-mcp-kernel-death-test";
    path = testsRoot + "/test_kernel_death.py";
  };
  kernelDeathSmoke =
    pkgs.runCommand "ix-mcp-kernel-death-smoke"
    {
      nativeBuildInputs = [typecheckTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      cp ${kernelDeathTestSource} "$TMPDIR/test_kernel_death.py"
      ${lib.getExe typecheckTestPython} -m pytest "$TMPDIR/test_kernel_death.py" -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp kernel-death smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';
  # An INTENTIONAL restart (the kernel_restart tool, index#2345) must be
  # surgical: only this server's kernel child is bounced (old pid -> new pid,
  # elapsed time reported), the namespace is rebuilt, the session name/topic the
  # server pushed are re-applied, and stderr carries the requested-restart lines
  # -- never the death watch's `kernel died` report, since the kill is on
  # purpose (packages/mcp/tests/test_kernel_restart.py). Boots a real kernel,
  # so it reuses the full interpreter plus pytest.
  kernelRestartTestSource = builtins.path {
    name = "ix-mcp-kernel-restart-test";
    path = testsRoot + "/test_kernel_restart.py";
  };
  kernelRestartSmoke =
    pkgs.runCommand "ix-mcp-kernel-restart-smoke"
    {
      nativeBuildInputs = [typecheckTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      cp ${kernelRestartTestSource} "$TMPDIR/test_kernel_restart.py"
      ${lib.getExe typecheckTestPython} -m pytest "$TMPDIR/test_kernel_restart.py" -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp kernel-restart smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';
  # python_exec's wedge escalation (index#2375): the SIGUSR2 rescue only helps a
  # Python-level block; a main thread stuck inside native code never runs the
  # handler, so after the interrupt the server must PROBE the kernel and, when
  # the probe hangs too, kill and respawn only the kernel child (never claim
  # "usable again" on mere signal delivery). Boots a real kernel, so it reuses
  # the full interpreter plus pytest.
  wedgeEscalationTestSource = builtins.path {
    name = "ix-mcp-wedge-escalation-test";
    path = testsRoot + "/test_kernel_wedge_escalation.py";
  };
  wedgeEscalationSmoke =
    pkgs.runCommand "ix-mcp-wedge-escalation-smoke"
    {
      nativeBuildInputs = [typecheckTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      cp ${wedgeEscalationTestSource} "$TMPDIR/test_kernel_wedge_escalation.py"
      ${lib.getExe typecheckTestPython} -m pytest "$TMPDIR/test_kernel_wedge_escalation.py" -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp wedge-escalation smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';
  # Exercises the rich-output capture path: a DataFrame result is persisted to the
  # store with its text/html bundle (so the dashboard renders a table, not a repr),
  # a display() call made while a job runs is captured the same way, and a bytes
  # image payload normalizes to a base64 string. Stands up an InteractiveShell
  # in-process so the formatter runs without booting a kernel; sandbox-safe.
  richTestPy = pkgs.writeText "ix-mcp-rich-test.py" ''
    # python
    import asyncio
    import json
    import os
    import sqlite3
    import tempfile

    from IPython.core.interactiveshell import InteractiveShell

    # A kernel always has a shell; this in-process test stands one up so the rich
    # formatter path runs without booting a kernel.
    InteractiveShell.instance()

    store_path = tempfile.mktemp(suffix=".db")
    os.environ["IX_MCP_STORE"] = store_path
    os.environ["WEAVE_URL"] = "off"

    import polars as pl

    from ix_notebook_mcp import runtime

    # A bytes image payload must normalize to a base64 string: raw bytes would not
    # survive JSON storage or an <img> data URI.
    bundle = runtime._normalize_bundle({"image/png": b"\x89PNG\r\n", "text/plain": "x"})
    assert isinstance(bundle["data"]["image/png"], str), bundle

    ns = {"pl": pl}
    runtime.install(ns)
    run = ns["__ix_run"]

    # The model-facing view (IX_LLM_MIME: the exact llm_result text plus downscaled
    # images) rides into the stored bundle so the dashboard's raw-LLM toggle can
    # show precisely what the agent received, not just the human HTML.
    llm_bundle = runtime._result_bundle(
        runtime.Result(user_html="<b>chart</b>", llm_result="a chart of x", llm_images=[b"\x89PNG\r\n"])
    )
    assert runtime.IX_LLM_MIME in llm_bundle["data"], list(llm_bundle["data"])
    decoded = json.loads(llm_bundle["data"][runtime.IX_LLM_MIME])
    assert decoded["text"] == "a chart of x" and len(decoded["images"]) == 1, decoded
    # A result with no model images still carries IX_LLM_MIME so to_mcp can prefer
    # the explicit model view over any human HTML fallback.
    plain_bundle = runtime._result_bundle(runtime.Result(user_html="<b>hi</b>", llm_result="hi"))
    assert json.loads(plain_bundle["data"][runtime.IX_LLM_MIME]) == {"text": "hi", "images": []}
    # A huge llm_result is clipped to the same cap as any other text mime, so it
    # can never bypass the limit into the store / each dashboard poll.
    big = runtime._result_bundle(
        runtime.Result(user_html="<b>x</b>", llm_result="z" * 500000, llm_images=[b"\x89PNG\r\n"])
    )
    big_text = json.loads(big["data"][runtime.IX_LLM_MIME])["text"]
    assert big_text.endswith("[truncated]") and len(big_text) <= runtime._MAX_TEXT_BUNDLE + 32, len(big_text)

    # A tuple/list carrying a rich value (a DataFrame) renders each element with
    # its own view, stacked, instead of stringifying the frame into a one-column
    # table -- Result((repr_text, df)) shows the text AND the real table.
    stacked = runtime.Result.of(("GrepResult: 0 matches", pl.DataFrame({"a": [1, 2]})))
    assert stacked.user_html.count("<table") == 1, stacked.user_html[:200]
    assert "GrepResult: 0 matches" in stacked.llm_result and "shape:" in stacked.llm_result, stacked.llm_result
    direct_df = runtime.Result.of(pl.DataFrame({"a": [1, 2], "b": ["x", "y"]}))
    assert '[[a, b]; [1, "x"], [2, "y"]]' in direct_df.llm_result, direct_df.llm_result
    assert "┌" not in direct_df.llm_result, direct_df.llm_result
    # A plain list of scalars is still ONE table (not stacked), unchanged.
    scalars = runtime.Result.of([1, 2, 3])
    assert scalars.user_html.count("<table") == 1, scalars.user_html[:200]
    # Stacking preserves a nested Result's model images (Result.of copies a
    # Result faithfully instead of rebuilding it from its display bundle).
    inner = runtime.Result(user_html="<b>x</b>", llm_result="x", llm_images=[b"\x89PNG\r\n"])
    nested = runtime.Result.of([inner, pl.DataFrame({"a": [1]})])
    assert len(nested.llm_images) == 1, ("nested Result dropped its images", nested.llm_images)

    # The table protocol: a non-DataFrame value exposing _ix_to_frame_() renders
    # as its polars frame: a styled table for the human, compact NUON for the
    # model -- instead of its one-line summary repr, so a rich result type shows
    # the model the real rows, not just a count.
    class _Framed:
        def _ix_to_frame_(self):
            return pl.DataFrame({"path": ["a.py"], "line": [3]})

        def __repr__(self):
            return "Framed: 1 match (summary)"

    framed = runtime.Result.of(_Framed())
    assert "<table" in framed.user_html, framed.user_html[:200]
    assert framed.llm_result.startswith("shape: (1, 2)") and "a.py" in framed.llm_result, framed.llm_result
    assert "summary" not in framed.llm_result, "must render the frame, not the summary repr"

    # Jupyter-style rich hooks can split a bare object's human HTML from its
    # model-facing text without manually constructing Result(...).
    class _Widget:
        def _repr_html_(self):
            return "<strong>human</strong>"

        def _repr_llm_(self):
            return "model-view"

    split = runtime.Result.of(_Widget())
    assert split.user_html == "<strong>human</strong>", split.user_html
    assert split.llm_result == "model-view", split.llm_result
    assert runtime._nuon([{"a": 1, "b": 2}, {"a": 5, "b": 7}]) == "[[a, b]; [1, 2], [5, 7]]"

    # A hook that raises or returns a non-frame is ignored: fall back to the
    # normal repr path rather than blowing up the result.
    class _BadFrame:
        def _ix_to_frame_(self):
            raise RuntimeError("nope")

    assert "BadFrame" in runtime.Result.of(_BadFrame()).llm_result

    # A plain string is rendered as output, not a Python literal: the model gets
    # it verbatim with terminal escapes stripped (no `\n` / `\x1b` repr noise),
    # and the human gets the same text as an HTML <pre>, escaped, with no raw
    # control bytes. This is the read-tool treatment for a streamed Result.
    s = runtime.Result.of("line1\nline2\n\x1b[0;32mgreen\x1b[0m")
    assert s.llm_result == "line1\nline2\ngreen", repr(s.llm_result)
    assert "\x1b" not in s.user_html and s.user_html.startswith("<pre"), s.user_html[:80]
    # A short string carries no surrounding repr quotes.
    assert runtime.Result.of("hello").llm_result == "hello"
    # HTML metacharacters are escaped for the human, verbatim for the model.
    esc = runtime.Result.of("a <b> & c")
    assert esc.llm_result == "a <b> & c" and "&lt;b&gt;" in esc.user_html, esc.user_html
    # An explicit llm_result still overrides the verbatim model text.
    assert runtime.Result.of("raw", llm_result="override").llm_result == "override"
    # The shared ANSI stripper lives in the runtime (the bundled `sh` helper
    # imports it rather than keeping a second copy).
    assert runtime._strip_ansi("\x1b[31mx\x1b[0m") == "x"


    async def main():
        # A DataFrame result is stored with its text/html bundle.
        df_job = await run("Result.of(pl.DataFrame({'a': [1, 2], 'b': ['x', 'y']}))", budget=3.0, name="df")
        await df_job.task
        assert df_job.status == "done", df_job.status
        result_mimes = {mime for out in runtime._job_outputs(df_job) for mime in out["data"]}
        assert "text/html" in result_mimes, ("result mimes", result_mimes)

        # An htpy element renders through the __html__ protocol: IPython's html
        # formatter ignores __html__ by default, so without _register_rich_formatters
        # cells.add/Result.of would store the element's repr instead of its HTML.
        htpy_job = await run(
            "import htpy\nResult.of(htpy.div(class_='x')['<hi>'])", budget=3.0, name="htpy"
        )
        await htpy_job.task
        htpy_html = [out["data"].get("text/html") for out in runtime._job_outputs(htpy_job)][-1]
        assert htpy_html == '<div class="x">&lt;hi&gt;</div>', htpy_html

        # A display() call made while a job runs is captured too.
        disp_job = await run(
            "from IPython.display import display\ndisplay(pl.DataFrame({'z': [9]}))\nResult.ok('shown')",
            budget=3.0,
            name="disp",
        )
        await disp_job.task
        disp_mimes = {mime for out in runtime._job_outputs(disp_job) for mime in out["data"]}
        assert "text/html" in disp_mimes, ("display mimes", disp_mimes)

        # A Result splits the human view (HTML on the dashboard) from the model
        # view (text in the tool result): the stored bundle carries user_html as
        # text/html, and to_mcp hands the model only the text/plain llm_result.
        from ix_notebook_mcp import outputs
        res_job = await run("Result(user_html='<b>hi</b>', llm_result='just-text')", budget=3.0, name="res")
        await res_job.task
        res_bundle = [out["data"] for out in runtime._job_outputs(res_job)][-1]
        assert res_bundle.get("text/html") == "<b>hi</b>", res_bundle
        mcp = outputs.to_mcp([{"output_type": "execute_result", "data": res_bundle, "metadata": {}}])
        texts = [c.text for c in mcp if getattr(c, "text", None) is not None]
        assert texts == ["just-text"], texts

        # Result DWIM: a bare value renders like Result.of (no user_html boilerplate).
        # A dict becomes a table -- a valid text/html string, not a raw dict that
        # breaks nbformat -- and its keys reach the model text.
        dwim_job = await run("Result({'alpha': 1, 'beta': 2})", budget=3.0, name="dwim")
        await dwim_job.task
        assert dwim_job.status == "done", dwim_job.status
        dwim_bundle = [out["data"] for out in runtime._job_outputs(dwim_job)][-1]
        assert isinstance(dwim_bundle.get("text/html"), str) and dwim_bundle["text/html"], dwim_bundle
        assert "alpha" in dwim_bundle.get("text/plain", "") and "beta" in dwim_bundle["text/plain"], dwim_bundle

        # Multiple values are ALL shown (not silently collapsed to the first).
        multi_job = await run("Result(True, [1, 2, 3])", budget=3.0, name="multi")
        await multi_job.task
        assert multi_job.status == "done", multi_job.status
        multi_text = [out["data"].get("text/plain", "") for out in runtime._job_outputs(multi_job)][-1]
        # Both values are shown: the bool by its repr, the list as its one-column
        # frame (NUON rows 1/2/3), not collapsed to just the first value.
        assert "true" in multi_text and "[[value]; [1], [2], [3]]" in multi_text, ("multi-value dropped a value", multi_text)


    asyncio.run(main())
    print("rich-ok")
  '';
  # Proves the yielding-cell behavior end to end: a cell that `yield`s streams
  # every yielded value to the store (the dashboard) and to the model (to_mcp),
  # keeps its top-level names in the namespace like a normal cell, and a
  # non-Result yield renders through Result.of. A plain (non-yielding) cell is
  # unchanged. In process (a shell, the store), no kernel boot or network, so
  # the sandbox runs it.
  yieldTestPy = pkgs.writeText "ix-mcp-yield-test.py" ''
    # python
    import asyncio
    import json
    import os
    import sqlite3
    import tempfile

    from IPython.core.interactiveshell import InteractiveShell

    InteractiveShell.instance()

    store_path = tempfile.mktemp(suffix=".db")
    os.environ["IX_MCP_STORE"] = store_path
    os.environ["WEAVE_URL"] = "off"

    from ix_notebook_mcp import outputs, runtime

    ns = {}
    runtime.install(ns)
    run = ns["__ix_run"]


    async def main():

        # A yielding cell streams multiple Results; its top-level names persist.
        code = (
            "acc = 0\n"
            "for i in range(3):\n"
            "    acc += i\n"
            "    yield Result.ok(f'step {i}')\n"
            "yield Result.of(acc)"
        )
        job = await run(code, budget=3.0, name="yield")
        await job.task
        assert job.status == "done", (job.status, job.error)
        assert ns["acc"] == 3, ns.get("acc")
        outs = runtime._job_outputs(job)
        htmls = [o["data"].get("text/html") for o in outs if "text/html" in o["data"]]
        assert len(htmls) == 4, ("expected 4 yielded results", len(htmls), outs)

        # Each yielded Result reaches the model: to_mcp over the stored bundles
        # hands back the llm text for every one.
        mcp = outputs.to_mcp(
            [{"output_type": "display_data", "data": o["data"], "metadata": {}} for o in outs]
        )
        texts = [c.text for c in mcp if getattr(c, "text", None) is not None]
        assert "step 0" in texts and "3" in texts, texts

        # A non-Result yield streams too: any value renders through Result.of,
        # exactly like a trailing expression.
        bare = await run("yield 123", budget=3.0, name="bare")
        await bare.task
        assert bare.status == "done", (bare.status, bare.error)
        bare_outs = runtime._job_outputs(bare)
        bare_mcp = outputs.to_mcp(
            [{"output_type": "display_data", "data": o["data"], "metadata": {}} for o in bare_outs]
        )
        bare_texts = [c.text for c in bare_mcp if getattr(c, "text", None) is not None]
        assert any("123" in t for t in bare_texts), bare_texts

        # A normal (non-yielding) cell is unchanged.
        plain = await run("Result.ok('plain')", budget=3.0, name="plain")
        await plain.task
        assert plain.status == "done", (plain.status, plain.error)


    asyncio.run(main())
    print("yield-ok")
  '';
  yieldSmoke =
    pkgs.runCommand "ix-mcp-yield-smoke"
    {
      nativeBuildInputs = [serverTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      ${lib.getExe serverTestPython} ${yieldTestPy} >stdout 2>stderr || {
        echo "ix-mcp yield smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      grep -qx 'yield-ok' stdout || {
        echo "ix-mcp yield smoke did not confirm yielded results:" >&2
        cat stdout stderr >&2
        exit 1
      }
      mkdir -p "$out"
    '';
  richSmoke =
    pkgs.runCommand "ix-mcp-rich-smoke"
    {
      nativeBuildInputs = [serverTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      ${lib.getExe serverTestPython} ${richTestPy} >stdout 2>stderr || {
        echo "ix-mcp rich smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      grep -qx 'rich-ok' stdout || {
        echo "ix-mcp rich smoke did not confirm rich-output capture:" >&2
        cat stdout stderr >&2
        exit 1
      }
      mkdir -p "$out"
    '';
  # Exercises the live-value introspection that feeds the dashboard's hover/inlay:
  # describe() classifies scalars, DataFrames, functions (with a source location),
  # and modules; cell_bindings() resolves a cell's mentioned names against the
  # namespace (excluding attribute parts); and a finished job persists those
  # bindings to the store, which is where the dashboard reads them. In-process, no
  # kernel or network, so the sandbox runs it.
  bindingsTestPy = pkgs.writeText "ix-mcp-bindings-test.py" ''
    # python
    import asyncio
    import inspect
    import json
    import os
    import sqlite3
    import tempfile

    import polars as pl

    from ix_notebook_mcp import introspect

    # Direct descriptors: each kind carries the inlay summary the dashboard shows.
    assert introspect.describe(42)["summary"] == "42"
    df_desc = introspect.describe(pl.DataFrame({"a": [1, 2, 3], "b": ["x", "y", "z"]}))
    assert df_desc["kind"] == "dataframe" and "3×2" in df_desc["summary"], df_desc

    # A wide frame's schema detail is capped, not dumped whole, so the stored row
    # and poll payload stay bounded.
    wide = introspect.describe(pl.DataFrame({f"c{i}": [0] for i in range(30)}))
    assert "+6 more" in wide["detail"], wide

    def sample(x):
        "a doc line"
        return x

    fn_desc = introspect.describe(sample)
    assert fn_desc["kind"] == "callable" and fn_desc["summary"].startswith("ƒ sample"), fn_desc
    # A function has a definition site: this is the go-to-definition payload.
    assert ":" in fn_desc.get("def", ""), fn_desc

    mod_desc = introspect.describe(inspect)
    assert mod_desc["kind"] == "module" and mod_desc["summary"] == "module inspect", mod_desc

    # cell_bindings resolves names a cell mentions; an attribute (df.height) is not
    # a name, so only `df` and `n` are described, not `height`.
    ns = {"df": pl.DataFrame({"a": [1]}), "n": 7}
    bound = introspect.cell_bindings("rows = df.height\ntotal = n + 1\n", ns)
    assert set(bound) == {"df", "n"}, bound
    assert bound["df"]["kind"] == "dataframe" and bound["n"]["summary"] == "7", bound

    # End to end: a finished job snapshots the bindings that persistence emits.
    store_path = tempfile.mktemp(suffix=".db")
    os.environ["IX_MCP_STORE"] = store_path
    os.environ["WEAVE_URL"] = "off"

    from IPython.core.interactiveshell import InteractiveShell

    InteractiveShell.instance()

    from ix_notebook_mcp import runtime

    user_ns = {"pl": pl}
    runtime.install(user_ns)
    run = user_ns["__ix_run"]


    async def main():
        job = await run("frame = pl.DataFrame({'a': [1, 2]})\nResult.ok('made it')", budget=3.0, name="bind")
        await job.task
        stored = runtime._cell_bindings(job)
        assert stored.get("frame", {}).get("kind") == "dataframe", stored
        # `pl` is referenced and live, so it is described as a module.
        assert stored.get("pl", {}).get("kind") == "module", stored


    asyncio.run(main())
    print("bindings-ok")
  '';
  bindingsSmoke =
    pkgs.runCommand "ix-mcp-bindings-smoke"
    {
      nativeBuildInputs = [serverTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      ${lib.getExe serverTestPython} ${bindingsTestPy} >stdout 2>stderr || {
        echo "ix-mcp bindings smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      grep -qx 'bindings-ok' stdout || {
        echo "ix-mcp bindings smoke did not confirm value introspection:" >&2
        cat stdout stderr >&2
        exit 1
      }
      mkdir -p "$out"
    '';
  # Property-based (Hypothesis) tests for the vdom()/read() snapshot helpers
  # (packages/mcp/tests/test_vdom_properties.py): they generate random HTML
  # bodies and assert selector integrity, exclusion of hidden/script/style
  # subtrees, name clamping, ref contiguity, determinism, the interactive_only
  # subset relation, geometry, and read()/vdom() agreement against a real
  # headless Chromium. Like browserVdomSmoke, vdom() only reads the DOM, so it
  # runs headless on set_content fixtures in the sandbox with no display or
  # network. The interpreter carries the browser closure + playwright (base)
  # plus pytest and hypothesis, which the bare test envs omit. The test file
  # reaches `browser` through pytest.importorskip, so the module must be in the
  # env or the whole suite silently skips.
  vdomTestPython = bundledTestPythonWith (ps: [
    ps.pytest
    ps.hypothesis
  ]) [browserModule];
  vdomPropertiesSource = builtins.path {
    name = "ix-mcp-vdom-properties-test";
    path = testsRoot + "/test_vdom_properties.py";
  };
  vdomPropertiesSmoke =
    pkgs.runCommand "ix-mcp-vdom-properties-smoke"
    {
      nativeBuildInputs = [vdomTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      # `vdom()` launches a (headless) browser; point Playwright at the bundled
      # browser bundle (no wrapper sets it for the bare interpreter).
      export PLAYWRIGHT_BROWSERS_PATH=${lib.escapeShellArg playwrightBrowsers}
      export FONTCONFIG_FILE=${fontsConf}
      # Copy the test into a writable dir so pytest collects it as a plain file
      # (a bare store path of a single .py is read by pytest as a directory).
      cp ${vdomPropertiesSource} "$TMPDIR/test_vdom_properties.py"
      ${lib.getExe vdomTestPython} -m pytest "$TMPDIR/test_vdom_properties.py" -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp vdom property tests failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';
  # Interactive input: the browser -> kernel write path behind interactive
  # resources (packages/mcp/tests/test_inputs.py). Covers the store queue, the
  # dashboard `/api/input` gate + CORS (over a real aiohttp TestServer), and the
  # kernel-side drain into an awaiting `Input` / `ask`. Needs the server
  # closure (ix_notebook_mcp + aiohttp) plus pytest, which bare test envs omit.
  inputsTestPython = bundledTestPythonWith (ps: [ps.pytest]) [ixNotebookMcpModule];
  inputsTestSource = builtins.path {
    name = "ix-mcp-inputs-test";
    path = testsRoot + "/test_inputs.py";
  };
  inputsTests =
    pkgs.runCommand "ix-mcp-inputs-tests"
    {
      nativeBuildInputs = [inputsTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      cp ${inputsTestSource} "$TMPDIR/test_inputs.py"
      ${lib.getExe inputsTestPython} -m pytest "$TMPDIR/test_inputs.py" -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp inputs tests failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';
  # Background-task failure reporting (packages/mcp/tests/test_task_errors.py):
  # a fire-and-forget task that dies with an unretrieved exception must be
  # reported at completion into `task_errors` (asyncio's own warning only fires
  # at GC, and never for a task a namespace variable keeps alive -- the exact
  # watcher pattern that starved monitors silently on 2026-07-02), plus the
  # `Result.output` alias that AttributeError'd that watcher.
  taskErrorsTestSource = builtins.path {
    name = "ix-mcp-task-errors-test";
    path = testsRoot + "/test_task_errors.py";
  };
  taskErrorsTests =
    pkgs.runCommand "ix-mcp-task-errors-tests"
    {
      nativeBuildInputs = [channelTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      cp ${taskErrorsTestSource} "$TMPDIR/test_task_errors.py"
      ${lib.getExe channelTestPython} -m pytest "$TMPDIR/test_task_errors.py" -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp task-errors tests failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';
  # Redundant-read tracking (packages/mcp/tests/test_readstats.py, index#1924):
  # the per-session tracker's core contract (same-content re-read is redundant,
  # changed content is not, a different path is not, counters are per-session) and
  # the exact `mcp_read_stats` journald line the ix fleet pipeline parses. Only
  # imports `ix_notebook_mcp.readstats` (pure stdlib), so it reuses the typecheck
  # interpreter, which already carries pytest.
  readStatsTestSource = builtins.path {
    name = "ix-mcp-readstats-test";
    path = testsRoot + "/test_readstats.py";
  };
  readStatsTests =
    pkgs.runCommand "ix-mcp-readstats-tests"
    {
      nativeBuildInputs = [typecheckTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      cp ${readStatsTestSource} "$TMPDIR/test_readstats.py"
      ${lib.getExe typecheckTestPython} -m pytest "$TMPDIR/test_readstats.py" -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp readstats tests failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';
  # The Claude Code channel + interactive resource actions
  # (packages/mcp/tests/test_channel.py): the store outbox/events queues, the
  # kernel's notify() + action dispatch, the transport pump emitting
  # notifications/claude/channel, the reply tool, and the dashboard's SSE feed.
  # Same interpreter needs as inputsTests (ix_notebook_mcp + aiohttp + the mcp
  # SDK) plus pytest.
  channelTestPython = bundledTestPythonWith (ps: [ps.pytest]) [ixNotebookMcpModule];
  channelTestSource = builtins.path {
    name = "ix-mcp-channel-test";
    path = testsRoot + "/test_channel.py";
  };
  channelTests =
    pkgs.runCommand "ix-mcp-channel-tests"
    {
      nativeBuildInputs = [channelTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      cp ${channelTestSource} "$TMPDIR/test_channel.py"
      ${lib.getExe channelTestPython} -m pytest "$TMPDIR/test_channel.py" -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp channel tests failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';
  # The MCP Apps view mechanism (packages/mcp/tests/test_mcp_ui.py): the
  # `ui://` viewer resource's spec shape (mimeType, lifecycle markers), the
  # tool `_meta.ui.resourceUri` linkage, `ui_result`'s content-preserving
  # `_meta` payload (proved over a real in-memory MCP session), the fragment
  # extraction/budget, and the data API's `/api/jobs/{id}/ui` embedded view
  # the room's sandboxed iframe loads. Same interpreter needs as channelTests
  # (ix_notebook_mcp + the mcp SDK + aiohttp) plus pytest.
  mcpUiTestSource = builtins.path {
    name = "ix-mcp-ui-test";
    path = testsRoot + "/test_mcp_ui.py";
  };
  mcpUiTests =
    pkgs.runCommand "ix-mcp-ui-tests"
    {
      nativeBuildInputs = [channelTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      cp ${mcpUiTestSource} "$TMPDIR/test_mcp_ui.py"
      ${lib.getExe channelTestPython} -m pytest "$TMPDIR/test_mcp_ui.py" -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp MCP Apps view tests failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';
  # End-to-end browser proof of the interactive-input path: a real headless
  # Chromium mounts an `Input`'s HTML in a sandboxed, opaque-origin srcdoc iframe
  # (as HtmlBody.svelte does), clicks the button, and the cross-origin `ixSubmit`
  # fetch must reach the real aiohttp `/api/input` and drain into the awaiting
  # channel (packages/mcp/tests/test_input_browser.py). Same interpreter + bundled
  # browser as the vdom smoke, plus pytest.
  inputBrowserTestPython = bundledTestPythonWith (ps: [ps.pytest]) [ixNotebookMcpModule];
  inputBrowserTestSource = builtins.path {
    name = "ix-mcp-input-browser-test";
    path = testsRoot + "/test_input_browser.py";
  };
  inputBrowserSmoke =
    pkgs.runCommand "ix-mcp-input-browser-smoke"
    {
      nativeBuildInputs = [inputBrowserTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      export PLAYWRIGHT_BROWSERS_PATH=${lib.escapeShellArg playwrightBrowsers}
      export FONTCONFIG_FILE=${fontsConf}
      cp ${inputBrowserTestSource} "$TMPDIR/test_input_browser.py"
      ${lib.getExe inputBrowserTestPython} -m pytest "$TMPDIR/test_input_browser.py" -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp input browser smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';
  # Network-free unit tests for the federated-resources bridge: every path of
  # `resources_bridge` (list/read/act, peer-flag assembly, not-found -> -32002,
  # graceful empty/clear-error when `ix-resource-cli` is absent) driven against a
  # STUB `ix-resource-cli` script on PATH plus a nonexistent-binary path -- no
  # real CLI or peer needed. The bridge lives in the `ix_notebook_mcp` server
  # package, so the test imports that module (bundled here) rather than a `src/*`
  # helper; `bash` is on PATH for the stub script's shebang.
  resourcesBridgeTestPython = pkgs.python3.withPackages (ps: [
    ps.pytest
    ps.pydantic
    ixNotebookMcpModule
  ]);
  resourcesBridgeTestSource = builtins.path {
    name = "ix-mcp-resources-bridge-test";
    path = testsRoot + "/test_resources_bridge.py";
  };
  resourcesBridgeTests =
    pkgs.runCommand "ix-mcp-resources-bridge-tests"
    {
      nativeBuildInputs = [
        resourcesBridgeTestPython
        pkgs.bash
      ];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      cp ${resourcesBridgeTestSource} "$TMPDIR/test_resources_bridge.py"
      ${lib.getExe resourcesBridgeTestPython} -m pytest "$TMPDIR/test_resources_bridge.py" -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp resources-bridge tests failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';
in {
  module = ixNotebookMcpModule;
  tests = {
    inherit
      bindDefaultSmoke
      sshAuthSockSmoke
      runtimeSmoke
      sessionSmoke
      wedgeSmoke
      kernelDeathSmoke
      kernelRestartSmoke
      wedgeEscalationSmoke
      yieldSmoke
      richSmoke
      bindingsSmoke
      vdomPropertiesSmoke
      inputsTests
      taskErrorsTests
      readStatsTests
      channelTests
      mcpUiTests
      inputBrowserSmoke
      resourcesBridgeTests
      ;
  };
}
