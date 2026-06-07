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

stdout and stderr are merged in emission order (terminal-style). A non-zero exit
is surfaced, never swallowed: the model view appends an ``[exit N]`` marker, and
``await sh(cmd, check=True)`` raises :class:`ShellError` instead of returning.
"""

from __future__ import annotations

import asyncio
import html as _html
import os
import re
import shlex
import signal

__all__ = ["sh", "Output", "ShellError"]

__version__ = "0.1.0"

# `Result` is the kernel runtime's human/model split. Importing it lets an
# `Output` BE a Result, so a cell can end with `await sh(...)` and satisfy the
# contract with no `Result.of(...)` wrapper. Outside the kernel (plain `import
# sh` in a script or a test) the runtime is absent; fall back to `object` so the
# module still imports and `_repr_html_`/`__repr__` carry the rendering.
try:
    from ix_notebook_mcp.runtime import Result as _ResultBase

    _HAS_RESULT = True
except Exception:  # pragma: no cover - exercised only outside the kernel
    _ResultBase = object
    _HAS_RESULT = False

# Strip the terminal escape families a CLI actually emits, not just CSI color.
# With FORCE_COLOR forced on, tools like `gh`/`eza`/`ls` emit OSC-8 hyperlinks
# and charset-reset (`ESC ( B`) around their output; matching CSI alone would
# leak the `\x1b` bytes of those into the model's text. Order matters: the
# string-terminated families (OSC/DCS) come before the single-final forms so an
# introducer is never half-matched.
_ANSI = re.compile(
    r"\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)"  # OSC string, BEL- or ST-terminated
    r"|\x1b[P^_X][^\x1b]*\x1b\\"  # DCS/PM/APC/SOS string, ST-terminated
    r"|\x1b\[[0-9;?]*[ -/]*[@-~]"  # CSI (color, cursor, mode)
    r"|\x1b[()*+#%][@-~]"  # charset designation / selection (e.g. ESC ( B)
    r"|\x1b[@-Z\\-_a-z=>]"  # remaining solo Fe/Fs escapes (RIS, keypad, ...)
)

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


def _strip_ansi(text: str) -> str:
    return _ANSI.sub("", text)


class ShellError(RuntimeError):
    """Raised by ``await sh(cmd, check=True)`` when the command exits non-zero.

    Carries the :class:`Output` so the failing command's text is still
    inspectable: ``except ShellError as e: print(e.output.text)``.
    """

    def __init__(self, output: "Output") -> None:
        self.output = output
        super().__init__(f"command exited {output.code}: {output.cmd}")


class Output(_ResultBase):
    """The result of one :func:`sh` call: a colored view for the human, escape-
    stripped text for the model.

    It is a ``Result`` subclass, so returning it as a cell's final expression
    renders ``user_html`` (the ANSI-to-HTML terminal block) on the dashboard and
    hands the model ``llm_result`` (the same output with escape codes removed).
    """

    def __init__(self, *, cmd: str, code: int, raw: str, duration: float) -> None:
        self.cmd = cmd
        self.code = code
        self.raw = raw
        self.duration = duration
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

    def lines(self) -> list[str]:
        """The escape-stripped output split into lines (trailing newline dropped)."""
        return self.text.splitlines()

    def _render_text(self) -> str:
        body = self.text
        if self.code == 0:
            return body
        # Flag a failure so the model never reads non-zero output as success.
        marker = f"[exit {self.code}]"
        return f"{body}\n{marker}" if body else marker

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


def _ansi_to_html(raw: str) -> str:
    """Render ANSI SGR color to inline-styled HTML, or escape plain text if the
    ``ansi2html`` converter is unavailable."""
    try:
        from ansi2html import Ansi2HTMLConverter
    except ImportError:
        # The converter is not installed (module used outside the bundled
        # interpreter): show the escape-stripped text rather than control bytes.
        return _html.escape(_strip_ansi(raw))
    conv = Ansi2HTMLConverter(inline=True, scheme="osx", dark_bg=True)
    return conv.convert(raw, full=False)


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
        try:
            proc.kill()
        except ProcessLookupError:
            pass


async def sh(
    cmd: str | list[str],
    *,
    cwd: str | os.PathLike | None = None,
    env: dict[str, str] | None = None,
    timeout: float | None = None,
    check: bool = False,
    color: bool = True,
) -> Output:
    """Run ``cmd`` on the shared async loop and return its :class:`Output`.

    ``cmd`` is a string (run through the shell, so pipes and globs work) or an
    argv list (executed directly, no shell parsing). stdout and stderr are merged
    in order. ``cwd`` and ``env`` extend the current directory and environment;
    ``timeout`` (seconds) kills the child's whole process group and raises
    :class:`TimeoutError`; ``check=True`` raises :class:`ShellError` on a non-zero
    exit; ``color=False`` suppresses the forced-color environment.

    With no ``timeout`` a command that keeps the stdout pipe open (a daemon it
    backgrounds, say) waits for that pipe to close. The await yields to the loop,
    so it never blocks other jobs; pass ``timeout`` to bound such a command.
    """
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

    loop = asyncio.get_running_loop()
    started = loop.time()
    try:
        if timeout is not None:
            stdout, _ = await asyncio.wait_for(proc.communicate(), timeout)
        else:
            stdout, _ = await proc.communicate()
    except asyncio.TimeoutError:
        _terminate(proc)
        # The group is dead, so the pipe closes and this reap returns promptly;
        # bound it anyway so a wedged reap can never hang the job past its timeout.
        try:
            await asyncio.wait_for(proc.wait(), 2.0)
        except asyncio.TimeoutError:
            pass
        raise TimeoutError(f"command timed out after {timeout}s: {shown}") from None

    duration = loop.time() - started
    out = Output(
        cmd=shown,
        code=proc.returncode if proc.returncode is not None else -1,
        raw=stdout.decode("utf-8", "replace"),
        duration=duration,
    )
    if check and not out.ok:
        raise ShellError(out)
    return out
