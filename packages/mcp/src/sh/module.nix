{
  bundledSource,
  bundledTestPython,
  ixNotebookMcpModule,
  lib,
  pkgs,
}: let
  # The kernel's process runner. The public `sh()`/`zsh()` are RETIRED (agents
  # shell out through `await nu(...)`); they stay importable as disabled shims
  # that raise a migration hint. The private `sh._exec` runs on the kernel's loop
  # (never blocks it like a bare subprocess.run) and returns an Output that IS a
  # Result, so the dashboard sees the command's ANSI color rendered to HTML while
  # the model gets the same text escape-stripped. Kernel internals (the
  # grep/find search helpers, worktree plumbing) use `_exec`. Pure Python over the
  # bundled ansi2html; cross-platform.
  shPythonSource = bundledSource {
    name = "ix-mcp-sh-python-source";
    path = ./.;
  };
  shModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-sh-python-module"
    {
      strictDeps = true;
      passthru.ixFirstPartyDeps = [ixNotebookMcpModule];
      meta.description = "Async shell-out helper bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/sh"
      mkdir -p "$site"
      cp -r ${shPythonSource}/sh/. "$site/"
    ''
  );
  # The sh module: runs a real subprocess on the loop and proves the human/model
  # split. The command emits ANSI color; the dashboard view (_repr_html_ /
  # user_html) must carry that color as HTML while the model view (repr /
  # llm_result) is escape-stripped. Also guards the Result contract (an Output is
  # a Result, so a cell can end with it), exit-code capture, and check=True.
  # Pure local subprocess over the bundled ansi2html, so the sandbox runs it.
  shTestPy = pkgs.writeText "ix-mcp-sh-test.py" ''
    # python
    import asyncio

    import sh
    from ix_notebook_mcp.runtime import Result


    async def main():
        # A command that emits an SGR color escape around its output.
        colored = await sh._exec(r"printf '\033[31mred\033[0m\n'", cwd=".")
        assert colored.ok and colored.code == 0, colored.code
        # Model view: no escape bytes, the word survives.
        assert "\x1b" not in colored.text and "red" in colored.text, repr(colored.text)
        assert "\x1b" not in colored.llm_result, repr(colored.llm_result)
        # Human view: color rendered to HTML (a styled span), no raw escapes.
        html = colored._repr_html_()
        assert "\x1b" not in html and "span" in html.lower(), html[:200]
        assert "color" in html.lower(), html[:200]
        # An Output IS a Result, so ending a cell with it satisfies the contract.
        assert isinstance(colored, Result), type(colored)
        # The ANSI helpers are the runtime's single implementation, imported
        # here rather than duplicated in the sh module.
        from ix_notebook_mcp import runtime as _rt

        assert sh._strip_ansi is _rt._strip_ansi and sh._ansi_to_html is _rt._ansi_to_html

        # argv form, and a non-zero exit is surfaced (not swallowed): typed
        # (.code with the .exit_code/.returncode aliases), falsy, and loud at
        # BOTH ends of the model view so a head-read of a long log sees the
        # failure as surely as a tail-read (issue #1766).
        failed = await sh._exec(["false"], cwd=".")
        assert not failed.ok and failed.code == 1, failed.code
        assert failed.exit_code == 1 and failed.returncode == 1, failed.exit_code
        assert bool(failed) is False, "a failed Output must be falsy"
        assert "[exit 1]" in failed.llm_result, failed.llm_result
        assert failed.llm_result.splitlines()[0].startswith("[exit 1]"), failed.llm_result
        # ...and even an output-less failure both leads and TRAILS with the
        # marker, so a tail-read never lands on command text.
        assert failed.llm_result.rstrip().endswith("\n[exit 1]"), failed.llm_result
        noisy = await sh._exec("echo diagnostic-text; exit 3", cwd=".")
        first, *rest = noisy.llm_result.splitlines()
        assert first.startswith("[exit 3]") and "exit 3" in first, noisy.llm_result
        assert noisy.llm_result.rstrip().endswith("[exit 3]"), noisy.llm_result
        # ...while .text stays the command's own output, marker-free, so
        # reading diagnostics off a failure is unchanged.
        assert noisy.text.strip() == "diagnostic-text", repr(noisy.text)
        assert "diagnostic-text" in "\n".join(rest), noisy.llm_result

        # Rendered command text is secret-redacted (#1769 post-merge P1): a
        # failing command whose STRING carries a credential shape must not leak
        # it into the model view, the ShellError message, or the dashboard
        # HTML; the raw command stays on .cmd. Fixture token is repeated
        # filler, not a real credential.
        tok = "tok9" * 10
        leak = await sh._exec(f"false Bearer {tok}", cwd=".")
        assert not leak.ok, leak.code
        assert tok not in leak.llm_result, leak.llm_result
        assert "[redacted:bearer_token]" in leak.llm_result.splitlines()[0], leak.llm_result
        assert leak.llm_result.splitlines()[0].split(": ", 1)[1].startswith("false"), (
            leak.llm_result)  # argv[0] survives redaction: still identifiable
        assert tok not in leak._repr_html_() and "[redacted:" in leak._repr_html_()
        assert tok in leak.cmd  # programmatic surface stays raw
        try:
            await sh._exec(f"false token={tok}", check=True, cwd=".")
        except sh.ShellError as exc:
            assert tok not in str(exc), str(exc)
            assert "token=[redacted:credential]" in str(exc), str(exc)
        else:
            raise SystemExit("expected ShellError from check=True")
        # A multi-line command collapses to ONE failure line (tail-reads land
        # on markers, not command fragments).
        multi = await sh._exec("false a \\\n  b", cwd=".")
        assert multi.llm_result.splitlines()[0].startswith("[exit 1]"), multi.llm_result
        assert multi.llm_result.rstrip().endswith("[exit 1]"), multi.llm_result

        # The expected-nonzero class (grep exiting 1 on no match) stays
        # workable: branch on .ok/.code and read .text, nothing raises.
        nomatch = await sh._exec("grep zzz-no-such /dev/null", cwd=".")
        assert not nomatch.ok and nomatch.code == 1, nomatch.code
        assert "[exit" not in nomatch.text, repr(nomatch.text)
        # grep also carries a structured-owner hint; it rides INSIDE the
        # failure markers, so the model text still ends with [exit N].
        assert "[hint:" in nomatch.llm_result, nomatch.llm_result
        assert nomatch.llm_result.rstrip().endswith("[exit 1]"), nomatch.llm_result

        # check=True turns a non-zero exit into a typed error carrying the output.
        try:
            await sh._exec("exit 3", check=True, cwd=".")
        except sh.ShellError as exc:
            assert exc.output.code == 3, exc.output.code
        else:
            raise SystemExit("expected ShellError on a non-zero exit with check=True")

        # An OSC-8 hyperlink (what gh/eza emit under FORCE_COLOR) is a non-CSI
        # escape: the stripper must remove its \x1b bytes too, not just SGR color.
        osc = await sh._exec(r"printf '\033]8;;https://x\033\\link\033]8;;\033\\\n'", cwd=".")
        assert "\x1b" not in osc.text and "link" in osc.text, repr(osc.text)
        assert "\x1b" not in osc.llm_result, repr(osc.llm_result)

        # A timeout must terminate the command's whole group and return promptly,
        # even when the command backgrounds a child that holds the stdout pipe
        # (the case where a naive kill + reap hangs forever).
        loop = asyncio.get_running_loop()
        start = loop.time()
        try:
            await sh._exec("sleep 30 & echo started; wait", timeout=0.5, cwd=".")
        except TimeoutError:
            pass
        else:
            raise SystemExit("expected TimeoutError from a command that outlives its timeout")
        elapsed = loop.time() - start
        assert elapsed < 10, f"timeout did not return promptly: {elapsed:.1f}s"

        # The PUBLIC entry points are retired: `sh()`, `sh.sh()`, `sh.zsh()`,
        # and calling the module all raise a migration hint pointing at
        # `await nu(...)`, so a stale transcript fails loudly rather than
        # shelling out. The private `_exec` (exercised above) is what remains.
        for call in (lambda: sh("printf hi"), lambda: sh.sh("printf hi"), lambda: sh.zsh("print hi")):
            try:
                await call()
            except RuntimeError as exc:
                assert "await nu" in str(exc), exc
            else:
                raise SystemExit("expected a disabled sh()/zsh() to raise a migration hint")

        # A direct runner handle for the composition/streaming checks below.
        direct = await sh._exec("printf hi", cwd=".")
        assert direct.ok and direct.text == "hi", repr(direct.text)

        # cwd defaults to the current directory: no required-kwarg TypeError.
        import os
        here = await sh._exec("pwd")
        assert here.ok and here.text.strip() == os.path.realpath(os.getcwd()), (
            here.text, os.getcwd())

        # An Output composes like its text: slice, concat, contains, len, str.
        assert direct[-1:] == "i" and direct[0] == "h", (direct[-1:], direct[0])
        assert direct + "!" == "hi!" and "say " + direct == "say hi"
        assert "hi" in direct and len(direct) == 2 and str(direct) == "hi"
        # Truthiness is success: empty-but-successful stays truthy (test
        # emptiness with len), and a failed Output is falsy (asserted above).
        assert bool(await sh._exec("true")) is True

        # Output streams to sys.stdout as it arrives (echo=True forces it outside
        # a kernel job), escape-stripped -- so a long command's log lands in the
        # job's pageable stdout even if the cell backgrounds before binding it.
        import contextlib
        import io
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            echoed = await sh._exec(r"printf '\033[31mstreamed\033[0m\n'", echo=True)
        assert "streamed" in buf.getvalue() and "\x1b" not in buf.getvalue(), repr(buf.getvalue())
        assert "streamed" in echoed.text, repr(echoed.text)
        # And echo stays off by default outside a kernel job.
        quiet = io.StringIO()
        with contextlib.redirect_stdout(quiet):
            await sh._exec("printf silent")
        assert quiet.getvalue() == "", repr(quiet.getvalue())
        # A failing command's stream also carries the failure line, so a watcher
        # paging a backgrounded job's stdout (jobs['<id>'].tail()) sees the
        # terminal state even if the Output is never bound (issue #1766).
        fbuf = io.StringIO()
        with contextlib.redirect_stdout(fbuf):
            await sh._exec("echo dying; exit 5", echo=True)
        assert "dying" in fbuf.getvalue(), repr(fbuf.getvalue())
        assert "[exit 5]" in fbuf.getvalue(), repr(fbuf.getvalue())

        # Cancelling the awaiting task kills the child's whole process group:
        # no orphan keeps running (or holding a lock) after a .cancel().
        import signal
        import tempfile
        pidfile = tempfile.mktemp()
        task = asyncio.ensure_future(sh._exec(f"echo $$ > {pidfile}; sleep 30", cwd="."))
        for _ in range(100):
            await asyncio.sleep(0.05)
            try:
                pid = int(open(pidfile).read().strip())
                break
            except (FileNotFoundError, ValueError):
                continue
        else:
            raise SystemExit("child never wrote its pidfile")
        task.cancel()
        try:
            await task
        except asyncio.CancelledError:
            pass
        await asyncio.sleep(0.3)
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            pass  # the group is dead, as required
        else:
            os.kill(pid, signal.SIGKILL)
            raise SystemExit(f"cancel orphaned the child (pid {pid} still alive)")

        # Structured stdout decodes straight to Python (the polars on-ramp).
        doc = await sh._exec("printf '%s' '{\"a\": 1, \"b\": [2, 3]}'", cwd=".")
        assert doc.json() == {"a": 1, "b": [2, 3]}, doc.json()
        rows = await sh._exec("printf '%s\\n%s\\n' '{\"n\": 1}' '{\"n\": 2}'", cwd=".")
        assert rows.jsonl() == [{"n": 1}, {"n": 2}], rows.jsonl()
        # A failed command raises ShellError from json(), never a decode error.
        try:
            (await sh._exec("echo nope; exit 4", cwd=".")).json()
        except sh.ShellError as exc:
            assert exc.output.code == 4, exc.output.code
        else:
            raise SystemExit("expected ShellError from json() on a non-zero exit")

        print("sh-ok", sh.__version__)


    asyncio.run(main())
  '';
  shTestPython = bundledTestPython [shModule];
  shSmoke =
    pkgs.runCommand "ix-mcp-sh-smoke"
    {
      nativeBuildInputs = [
        shTestPython
        pkgs.zsh
      ];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      ${lib.getExe shTestPython} ${shTestPy} >stdout 2>stderr || {
        echo "ix-mcp sh smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      grep -q '^sh-ok' stdout || {
        echo "ix-mcp sh smoke did not confirm the sh module:" >&2
        cat stdout stderr >&2
        exit 1
      }
      mkdir -p "$out"
    '';
in {
  module = shModule;
  tests = {
    inherit
      shSmoke
      ;
  };
}
