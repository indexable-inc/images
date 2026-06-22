"""Run a shell command on the kernel's async loop and render it two ways.

Bundled like ``view``/``fff``/``fleet`` so every session can ``import sh`` with no
setup. The point: when you genuinely need to shell out (a ``gh``/``git``/``nix``
invocation with no Python binding), do it without blocking the one shared event
loop and without leaking terminal escape codes into your own context.

    import sh
    out = await sh("gh run list --limit 5")
    out                       # last expr: dashboard shows the COLORED terminal
                              # block, you get the escape-stripped plain text

``sh`` is async (built on :func:`asyncio.create_subprocess_shell`), so it never
freezes the kernel the way a bare ``subprocess.run`` does. The value it returns is
an :class:`Output`, which is a ``Result`` subclass: ending a cell with it
satisfies the kernel's Result contract directly, the human watching the dashboard
sees the command's real ANSI color rendered to HTML, and the model's tool result
gets the same output with every escape sequence stripped. The two never cross.

Color is captured by telling the child it may emit it (``FORCE_COLOR=1`` /
``CLICOLOR_FORCE=1``) while still capturing through pipes, so modern tools
(``gh``, ``git``, ``cargo``, ``rg``, ``eza``) produce clean SGR color with none of
the cursor-movement noise a PTY would inject. Pass ``color=False`` to disable it.

The :class:`Output` also exposes the parts programmatically::

    out.code     # exit status (int)
    out.ok       # out.code == 0
    out.text     # combined stdout+stderr, escape codes stripped
    out.raw      # the same, with the original ANSI color preserved
    out.cmd      # the command that was run
    out.lines()  # out.text split into lines
    out.json()   # parse out.text as one JSON document
    out.jsonl()  # parse out.text as JSON Lines (one value per line)

For a command that emits structured output, decode it straight into a polars
DataFrame without a hand-written ``json.loads``::

    import polars as pl
    prs = (await sh("gh pr list --json number,title,state", cwd=".")).json()
    pl.DataFrame(prs)
    # JSON Lines (cargo --message-format json, nix --log-format internal-json) -> .jsonl()
    msgs = (await sh("cargo build --message-format json", cwd=".")).jsonl()

``json``/``jsonl`` raise :class:`ShellError` when the command failed, so a broken
``gh ... --json`` surfaces its real error instead of a confusing decode failure.

An ``Output`` also behaves like its text for the common string operations
(``out[-4000:]``, ``out + "..."``, ``"error" in out``, ``len(out)``,
``str(out)``), so composing command output needs no ``str(...)`` wrapping.

stdout and stderr are merged in emission order (terminal-style). A non-zero exit
is surfaced, never swallowed: the model view appends an ``[exit N]`` marker, and
``await sh(cmd, check=True)`` raises :class:`ShellError` instead of returning.

**Never pass prose through shell quoting.** Backticks in a string command are
run as command substitution by the shell even when the string was produced by
Python ``repr()`` (the backticks survive the quoting), and a multi-line string
repr'd as a single-quoted argument loses its newlines (they become literal
``\\n``). For any argument that contains prose -- a commit message, a PR body --
use the argv-list form ``sh(['git', 'commit', '-m', msg])`` so the argument is
passed verbatim with no shell parsing, or write the text to a file and use
``git commit -F <file>``.

Inside the kernel the child's output also streams to the running cell's stdout
as it arrives, so it lands in ``jobs['<id>'].output`` live: a long command's log
is pageable from the job even when the cell backgrounds (or is cancelled) before
the ``Output`` value is ever bound. Cancelling the task kills the child's whole
process group, never orphaning it.

Pass ``name=`` to label the job in the dashboard and the ``jobs`` dict, mirroring
the same parameter on ``python_exec``::

    build = await sh("nix build .#mcp ...", cwd=wt, timeout=600, name="nix-build-mcp")
"""

from __future__ import annotations

import asyncio
import codecs
import contextlib
import html as _html
import inspect as _inspect
import json as _json
import os
import re
import shlex
import signal
import sys
from typing import Any

__all__ = ["Output", "ShellError", "sh", "zsh"]

__version__ = "0.1.0"

# `Result` is the kernel runtime's human/model split. Importing it lets an
# `Output` BE a Result, so a cell can end with `await sh(...)` and satisfy the
# contract with no `Result.of(...)` wrapper. Outside the kernel (plain `import
# sh` in a script or a test) the runtime is absent; fall back to `object` so the
# module still imports and `_repr_html_`/`__repr__` carry the rendering.
try:
    from ix_notebook_mcp.runtime import Result as _ResultBase
    from ix_notebook_mcp.runtime import _ANSI, _ansi_to_html, _ix_current, _strip_ansi
    from ix_notebook_mcp.runtime import _rename_current_job

    _HAS_RESULT = True
except Exception:  # pragma: no cover - exercised only outside the kernel
    # Standalone (`import sh` with no kernel): degrade gracefully. The canonical
    # ANSI handling lives in the runtime; without it, strip nothing and merely
    # escape for HTML rather than reimplement the escape grammar here.
    _ResultBase = object
    _HAS_RESULT = False
    _ix_current = None
    _rename_current_job = None  # type: ignore[assignment]
    # SGR color only; the full escape grammar is the runtime's to own.
    _ANSI = re.compile(r"\x1b\[[0-9;]*m")

    def _strip_ansi(text: str) -> str:
        return _ANSI.sub("", text)

    def _ansi_to_html(text: str) -> str:
        return _html.escape(text)

# Environment that asks well-behaved CLIs to emit SGR color even though their
# stdout is a pipe, not a TTY. PAGER=cat keeps a tool that auto-pages (git, gh)
# from blocking forever on a captured stream.
_COLOR_ENV = {
    "FORCE_COLOR": "1",
    "CLICOLOR_FORCE": "1",
    "CLICOLOR": "1",
    "TERM": "xterm-256color",
    "GIT_PAGER": "cat",
    "PAGER": "cat",
}

_MONO = "ui-monospace,SFMono-Regular,Menlo,monospace"


# Local file listing / reading / searching has a bundled, polars-first owner.
# When a command reaches for one of these tools directly, the Output carries a
# one-line hint to the model naming the structured alternative. A hint, not an
# error: piping through grep on a remote ssh command (the first token is then
# `ssh`, not `grep`) or a genuinely odd local pipeline stays untouched.
_STRUCTURED_OWNER = {
    "ls": "view.ls() returns this directory as a polars frame (pre-imported)",
    "tree": "view.tree() returns the tree as a polars frame (pre-imported)",
    "cat": "view.cat() renders the file; the `read` tool reads it without flooding the dashboard",
    "head": "view.cat() / the `read` tool with start/end replace head",
    "tail": "view.cat() / the `read` tool with start/end replace tail",
    "grep": "await grep(pattern, root) (ripgrep-backed) returns matches as a polars frame",
    "rg": "await grep(pattern, root) (ripgrep-backed) returns matches as a polars frame",
    "find": "await find(ext=..., root=...) (fd-backed) returns matching files as a polars frame",
    "fd": "await find(ext=..., root=...) (fd-backed) returns matching files as a polars frame",
}


def _structured_hint(cmd: str | list[str]) -> str | None:
    """A redirect hint when ``cmd`` starts with a tool a bundled module owns."""
    if isinstance(cmd, str):
        first = cmd.strip().split(None, 1)[0] if cmd.strip() else ""
    else:
        first = str(cmd[0]) if cmd else ""
    return _STRUCTURED_OWNER.get(first.rsplit("/", 1)[-1])


class ShellError(RuntimeError):
    """Raised by ``await sh(cmd, check=True)`` when the command exits non-zero.

    Carries the :class:`Output` so the failing command's text is still
    inspectable: ``except ShellError as e: print(e.output.text)``.
    """

    def __init__(self, output: Output) -> None:
        self.output = output
        super().__init__(f"command exited {output.code}: {output.cmd}")


class Output(_ResultBase):
    """The result of one :func:`sh` call: a colored view for the human, escape-
    stripped text for the model.

    It is a ``Result`` subclass, so returning it as a cell's final expression
    renders ``user_html`` (the ANSI-to-HTML terminal block) on the dashboard and
    hands the model ``llm_result`` (the same output with escape codes removed).
    """

    def __init__(
        self, *, cmd: str, code: int, raw: str, duration: float, hint: str | None = None
    ) -> None:
        self.cmd = cmd
        self.code = code
        self.raw = raw
        self.duration = duration
        self.hint = hint
        if _HAS_RESULT:
            super().__init__(
                user_html=self._render_html(),
                llm_result=self._render_text(),
                llm_images=[],
            )

    @property
    def ok(self) -> bool:
        return self.code == 0

    @property
    def text(self) -> str:
        """Combined stdout+stderr with ANSI escape codes stripped."""
        return _strip_ansi(self.raw)

    @property
    def stdout(self) -> str:
        """Alias for ``.text``: the merged stdout+stderr with ANSI codes stripped.

        Streams are merged in emission order (terminal-style), so there is no
        separate stderr channel. This alias exists so the conventional
        subprocess attribute name works without a wasted AttributeError roundtrip;
        ``.text`` and ``.stdout`` are identical. For separate streams, redirect
        in the command, e.g. ``await sh("cmd 2>err.txt")`` and read the file.
        """
        return self.text

    @property
    def stderr(self) -> str:
        """Alias for ``.text``: stdout and stderr are merged in emission order.

        Returns the same value as ``.stdout`` and ``.text``. The streams cannot
        be separated after the fact; if you need stderr alone, redirect it in
        the command, e.g. ``await sh("cmd 2>&1 1>/dev/null")``.
        """
        return self.text

    @property
    def output(self) -> str:
        """Alias for ``.text``, matching ``jobs['<id>'].output`` on a live job.

        The docstring above and the kernel instructions teach
        ``jobs['<id>'].output`` as the way to read a run's stdout; this alias
        makes the direct ``(await sh(...))`` return symmetric so the same
        attribute works whether the call ran in the foreground or in a tracked
        background job.
        """
        return self.text

    def lines(self) -> list[str]:
        """The escape-stripped output split into lines (trailing newline dropped)."""
        return self.text.splitlines()

    def json(self) -> object:
        """Parse the command's output (``.text``) as a single JSON document.

        For a tool with a JSON mode (``gh ... --json``, ``cargo metadata``,
        ``nix eval --json``): ``(await sh(...)).json()`` hands back the decoded
        Python value, ready for ``pl.DataFrame(...)``. Raises :class:`ShellError`
        if the command exited non-zero (so the real failure surfaces, not a
        :class:`json.JSONDecodeError` over an error message), and
        :class:`json.JSONDecodeError` if the output is not valid JSON.

        Note that ``.text`` is the merged stdout+stderr stream (see ``.stdout``):
        a command that writes diagnostics to stderr will interleave them with the
        JSON and fail to decode, so silence it (``2>/dev/null``) or capture stderr
        separately (``2>err.txt``) when the tool is chatty on success.
        """
        if not self.ok:
            raise ShellError(self)
        return _json.loads(self.text)

    def jsonl(self) -> list:
        """Parse the output (``.text``) as JSON Lines: one value per non-empty line.

        For tools that stream line-delimited JSON (``cargo --message-format
        json``, ``nix ... --log-format internal-json``). Same non-zero guard as
        :meth:`json`; blank lines are skipped. As with :meth:`json`, ``.text`` is
        the merged stdout+stderr stream, so a non-JSON diagnostic line will raise
        :class:`json.JSONDecodeError`; redirect stderr away if the tool emits one.
        """
        if not self.ok:
            raise ShellError(self)
        return [_json.loads(line) for line in self.text.splitlines() if line.strip()]

    def df(self) -> object:
        """The command's JSON output as a polars DataFrame: the one-liner for any
        CLI with a JSON mode.

        ``(await sh("gh run list --json status,conclusion,displayTitle")).df()``
        hands back a frame ready to ``.filter`` / ``.sort`` / render, instead of
        TSV text to scrape. Accepts a top-level JSON array of objects, a single
        object (one row), or JSON Lines. Same non-zero guard as :meth:`json`, and
        the same merged-stream caveat: silence chatty stderr (``2>/dev/null``)
        when the tool writes diagnostics on success.
        """
        import polars as pl

        if not self.ok:
            raise ShellError(self)
        try:
            value = _json.loads(self.text)
        except _json.JSONDecodeError:
            value = [_json.loads(line) for line in self.text.splitlines() if line.strip()]
        if isinstance(value, dict):
            value = [value]
        return pl.DataFrame(value)

    def _render_text(self) -> str:
        body = self.text
        if self.code != 0:
            # Flag a failure so the model never reads non-zero output as success.
            marker = f"[exit {self.code}]"
            body = f"{body}\n{marker}" if body else marker
        if self.hint:
            # Model view only: the human's terminal block stays clean. The hint
            # teaches the structured alternative at the exact moment the weaker
            # tool was reached for, which survives instruction truncation.
            body = f"{body}\n[hint: {self.hint}]" if body else f"[hint: {self.hint}]"
        return body

    def _render_html(self) -> str:
        body = _ansi_to_html(self.raw)
        badge_color = "#7bd88f" if self.code == 0 else "#fc618d"
        badge = (
            f'<span style="color:{badge_color}">exit {self.code}</span>'
            f'<span style="color:#6a6a70"> · {self.duration:.2f}s</span>'
        )
        prompt = (
            f'<div style="color:#6a6a70;padding:6px 10px 0">'
            f'<span style="color:#7bd88f">$</span> '
            f'{_html.escape(self.cmd)}</div>'
        )
        out = (
            f'<pre style="margin:0;padding:6px 10px 10px;white-space:pre-wrap;'
            f'word-break:break-word">{body}</pre>'
        )
        foot = f'<div style="padding:0 10px 6px;font-size:11px">{badge}</div>'
        return (
            f'<div style="background:#141416;border:1px solid #242427;border-radius:6px;'
            f'color:#e6e6e6;font-family:{_MONO};font-size:12px;overflow:auto">'
            f"{prompt}{out}{foot}</div>"
        )

    def __repr__(self) -> str:
        return self._render_text()

    def _repr_html_(self) -> str:
        return self._render_html()

    # An Output composes like its text: slice it, concatenate it, search it,
    # measure it -- no `str(...)` wrapping. All delegate to `.text` (the
    # escape-stripped output), the same view `str(out)` returns.
    def __str__(self) -> str:
        return self.text

    def __bool__(self) -> bool:
        # Defining __len__ would otherwise make an empty (but successful) output
        # falsy; an Output is a result object, so it is always truthy -- test
        # success with `.ok`, emptiness with `len(out)`.
        return True

    def __getitem__(self, key: int | slice) -> str:
        return self.text[key]

    def __len__(self) -> int:
        return len(self.text)

    def __contains__(self, item: object) -> bool:
        return item in self.text

    def __add__(self, other: str) -> str:
        return self.text + other

    def __radd__(self, other: str) -> str:
        return other + self.text


def _terminate(proc: asyncio.subprocess.Process) -> None:
    """Kill the child and the process group it leads.

    ``sh`` starts each child in its own session (``start_new_session=True``), so a
    command that backgrounds a grandchild (which would otherwise keep the merged
    stdout pipe open and hang the reap forever) is killed as a group here.
    """
    try:
        os.killpg(os.getpgid(proc.pid), signal.SIGKILL)
    except (ProcessLookupError, PermissionError):
        # Process already gone, or no group to signal: kill the child directly.
        with contextlib.suppress(ProcessLookupError):
            proc.kill()


class _EchoStripper:
    """Incrementally strip ANSI escapes from streamed chunks.

    A chunk boundary can split an escape sequence in two; a naive per-chunk
    ``_strip_ansi`` would then leak half of it as visible garbage. This holds
    back a trailing, still-incomplete escape and prepends it to the next chunk,
    so the echoed stream is clean no matter where the pipe chops it.
    """

    def __init__(self) -> None:
        self._pending = ""

    def feed(self, text: str) -> str:
        text = self._pending + text
        self._pending = ""
        cut = text.rfind("\x1b")
        if cut != -1:
            tail = text[cut:]
            # A complete sequence (or ESC followed by plain text) strips fine;
            # only a short, genuinely unfinished introducer is held back.
            if _ANSI.match(tail) is None and len(tail) < 64:
                self._pending = tail
                text = text[:cut]
        return _strip_ansi(text)

    def flush(self) -> str:
        text, self._pending = self._pending, ""
        return _strip_ansi(text)


def _in_kernel_job() -> bool:
    """True when this call runs inside a kernel job, where ``sys.stdout`` routes
    to that job's captured output (the runtime's tee)."""
    return _ix_current is not None and _ix_current.get() is not None


async def sh(
    cmd: str | list[str],
    *,
    cwd: str | os.PathLike | None = None,
    env: dict[str, str] | None = None,
    timeout: float | None = None,
    check: bool = False,
    color: bool = True,
    echo: bool | None = None,
    name: str | None = None,
) -> Output:
    """Run ``cmd`` on the shared async loop and return its :class:`Output`.

    ``cmd`` is a string (run through the shell, so pipes and globs work) or an
    argv list (executed directly, no shell parsing). stdout and stderr are merged
    in order. ``cwd`` is the directory to run in (defaults to the kernel's
    current directory); pass it instead of a `cd X && ...` prefix, which is
    rejected, so the command string stays clean. ``env`` extends the environment;
    ``timeout`` (seconds) kills the child's whole process group and raises
    :class:`TimeoutError`; ``check=True`` raises :class:`ShellError` on a non-zero
    exit; ``color=False`` suppresses the forced-color environment.
    ``name`` sets a human-readable label for the running job in the dashboard and
    the ``jobs`` dict (mirrors the same parameter on ``python_exec``); outside
    the kernel it is accepted and silently ignored.

    Output STREAMS as it arrives: inside the kernel each chunk is echoed
    (escape-stripped) to the running cell's stdout, so a long command's log is in
    ``jobs['<id>'].output`` live and survives the cell backgrounding or being
    cancelled. ``echo`` overrides that default (it is off outside the kernel).
    Cancelling the awaiting task kills the child's whole process group, so a
    cancelled cell never leaves an orphan running (or holding a lock) behind.

    With no ``timeout`` a command that keeps the stdout pipe open (a daemon it
    backgrounds, say) waits for that pipe to close. The await yields to the loop,
    so it never blocks other jobs; pass ``timeout`` to bound such a command.

    Prefer structured output over text scraping: when the CLI has a JSON mode
    (``gh --json``, ``cargo metadata``, ``nix --json``) use it and parse with
    ``.json()`` / ``.jsonl()`` / ``.df()`` on the returned Output; ``.df()`` is a
    polars frame ready to filter and render. Run ONE command per call and combine
    the parsed results in Python, instead of chaining ``cmd1; echo ===; cmd2``
    and splitting text. For local listing / reading / searching, the bundled
    helpers are the owners (`view.ls`, `view.cat`, and the top-level
    `await grep(...)` / `await find(...)`); reaching for
    ``ls``/``cat``/``grep``/``find`` through the shell returns an Output carrying
    a hint to the structured alternative.

    Never pass prose through shell quoting: backticks in a string command are
    run as command substitution by the shell even when the string was produced
    by Python ``repr()`` (the backticks survive the quoting), and a multi-line
    message repr'd as a single-quoted string loses its newlines (they become
    literal ``\\n``). For any command argument that contains prose -- a commit
    message, a PR body, a description -- use the argv-list form
    ``sh(['git', 'commit', '-m', msg])`` so the argument is passed verbatim
    with no shell parsing, or write the text to a temporary file and pass
    ``git commit -F <file>``.
    """
    if isinstance(cmd, str) and re.match(r"\s*cd\b", cmd):
        raise ValueError(
            "sh() takes no `cd ...` prefix: pass the working directory as cwd= and keep "
            "the command itself clean, e.g. await sh('ix trace <id>', cwd='/path/to/repo')."
        )
    if isinstance(cmd, str) and "`" in cmd:
        raise ValueError(
            "sh(): backticks in a string command are shell command substitution -- they run "
            "even inside Python repr'd strings (the backticks survive repr quoting, then the "
            "shell executes them when it processes the argument). This is how `git commit -m "
            "{msg!r}` ended up executing `ix-mcp dashboard` and splicing its URL into the "
            "commit message. If you want $(...) substitution, write it as $(...) explicitly. "
            "If the backticks are prose (e.g. a commit message), use the argv-list form "
            "instead: sh(['git', 'commit', '-m', msg]) runs with no shell parsing and passes "
            "msg verbatim, or write the message to a temp file and use git commit -F <file>."
        )
    if isinstance(cmd, str) and re.search(
        r"git\s+commit\b.*\s(-m|--message)\s*['\"].*(?:\\n|\n).*['\"]", cmd, re.DOTALL
    ):
        raise ValueError(
            "sh(): a git commit -m/--message argument containing a newline (real or "
            "escaped \\n) will be flattened by the shell into a single line full of literal "
            r"'\n' characters when passed through Python repr. Use the argv-list form "
            "sh(['git', 'commit', '-m', msg]) to pass the message verbatim without shell "
            "parsing, or write it to a temp file and use git commit -F <file>."
        )
    if name is not None and _in_kernel_job() and _rename_current_job is not None:
        _rename_current_job(name)

    full_env = dict(os.environ)
    if color:
        full_env.update(_COLOR_ENV)
    if env:
        full_env.update(env)

    if isinstance(cmd, (list, tuple)):
        argv = [str(part) for part in cmd]
        shown = shlex.join(argv)
        proc = await asyncio.create_subprocess_exec(
            *argv,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.STDOUT,
            cwd=cwd,
            env=full_env,
            start_new_session=True,
        )
    else:
        shown = cmd
        proc = await asyncio.create_subprocess_shell(
            cmd,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.STDOUT,
            cwd=cwd,
            env=full_env,
            start_new_session=True,
        )

    do_echo = _in_kernel_job() if echo is None else echo
    decoder = codecs.getincrementaldecoder("utf-8")("replace")
    stripper = _EchoStripper()
    chunks: list[str] = []

    def _keep(text: str) -> None:
        chunks.append(text)
        if do_echo:
            sys.stdout.write(stripper.feed(text))

    async def _drain() -> None:
        while True:
            block = await proc.stdout.read(8192)
            if not block:
                break
            _keep(decoder.decode(block))
        tail = decoder.decode(b"", final=True)
        if tail:
            _keep(tail)
        if do_echo:
            sys.stdout.write(stripper.flush())
        await proc.wait()

    loop = asyncio.get_running_loop()
    started = loop.time()
    try:
        if timeout is not None:
            await asyncio.wait_for(_drain(), timeout)
        else:
            await _drain()
    except TimeoutError:
        _terminate(proc)
        # The group is dead, so the pipe closes and this reap returns promptly;
        # bound it anyway so a wedged reap can never hang the job past its timeout.
        with contextlib.suppress(TimeoutError):
            await asyncio.wait_for(proc.wait(), 2.0)
        raise TimeoutError(f"command timed out after {timeout}s: {shown}") from None
    except asyncio.CancelledError:
        # The awaiting task was cancelled (jobs['<id>'].cancel()): take the child
        # and its whole group down with it, so a cancelled cell never leaves an
        # orphan still running (and holding locks) in the background.
        _terminate(proc)
        raise

    duration = loop.time() - started
    out = Output(
        cmd=shown,
        code=proc.returncode if proc.returncode is not None else -1,
        raw="".join(chunks),
        duration=duration,
        hint=_structured_hint(cmd),
    )
    if check and not out.ok:
        raise ShellError(out)
    return out


async def zsh(cmd: str, **kwargs: Any) -> Output:
    """Run ``cmd`` through ``zsh -lc`` while keeping :func:`sh`'s safety wrapper.

    Use this only when the command intentionally depends on zsh syntax. For
    prose-bearing arguments, keep using ``sh(['prog', arg])`` so the shell never
    parses the argument. Pass ``cwd=`` instead of a leading ``cd``.
    """
    if re.match(r"\s*cd\b", cmd):
        raise ValueError(
            "zsh() takes no `cd ...` prefix: pass the working directory as cwd= and keep "
            "the command itself clean."
        )
    if "`" in cmd:
        raise ValueError(
            "zsh(): backticks are shell command substitution. Use $(...) when you "
            "intentionally want substitution, or sh([...]) when the text is prose."
        )
    return await sh(["zsh", "-lc", cmd], **kwargs)


# Make the module itself callable, so the documented `import sh; await sh(cmd)`
# works without reaching for `sh.sh`. The module object's class is swapped for a
# ModuleType subclass that forwards a call to the sh() coroutine function. The
# kernel binds this same module object as `sh` in the user namespace too (see
# ix_notebook_mcp.runtime.install), so `await sh(...)` works with or without an
# explicit import, while `sh.Output` / `sh.ShellError` stay reachable as attrs.
import types as _types


import functools as _functools


class _CallableModule(_types.ModuleType):
    @_functools.wraps(sh)
    def __call__(self, *args: object, **kwargs: object) -> object:
        if len(args) > 1:
            raise TypeError(
                "sh() takes one command argument. Pass argv as a single list, e.g. "
                "await sh(['git', 'status'], cwd=repo), not sh('git', 'status')."
            )
        return sh(*args, **kwargs)


_module = sys.modules[__name__]
_module.__class__ = _CallableModule
# `inspect.signature(callable_module)` inspects the bound __call__ method and
# would otherwise drop `cmd`. Publish the real callable signature so api() shows
# the load-bearing positional argument and the argv-list type.
_module.__signature__ = _inspect.signature(sh)  # type: ignore[attr-defined]
