"""An embedded nushell engine; every pipeline result is a polars DataFrame.

Bundled like ``view`` so every session can ``await nu(...)`` with no setup.
``nu`` is the ONE shell-out path (the old ``sh``/``zsh`` are retired). Nushell's
pipelines are structured end to end (``ls``, ``ps``, ``open``,
``from csv|toml|yaml``, ``where``, ``group-by``), so ``nu()`` is the bridge from
"shell pipeline" to "typed frame" for any data-shaped command, and an external
binary runs with ``^cmd``::

    df = await nu("ls | where size > 1kb | sort-by size")
    df = await nu("ps | where cpu > 5 | sort-by cpu")
    rec = await nu("open Cargo.toml | get package")         # record -> plain dict
    rec = await nu("http get https://api.github.com/repos/nushell/nushell")
    text = await nu("^git status --short")                  # external binary via ^ (stdout str)
    df = await nu("^gh pr list --json number,title | from json")  # JSON-mode CLI

This is not a subprocess: the engine (PyO3 bindings over nu-engine) lives in
this process and its state is persistent, like a REPL: a ``let``, a ``def``,
or a ``cd`` in one call is visible to the next. Each kernel session has its
own engine, so a ``cd`` here never moves another session's PWD (issue #2089:
the per-call re-sync to the shared process cwd silently redirected bare git
commands into other agents' worktrees). If the remembered directory has been
deleted, the next call fails loudly until you pass ``cwd=`` or ``nu.reset()``
(issue #1986)::

    await nu("let prs = (http get $url)")
    await nu("$prs | where author.login == 'andrewgazelka'")

A DataFrame (or list/dict/scalar) can be piped THROUGH a pipeline: pass
``input=`` and the code sees it as its pipeline input (``$in``)::

    df = await nu("where size > 1kb | sort-by size", input=df)

``nu()`` is the single shell-out path: side-effectful commands
(``git``/``gh`` writes) run as externals with ``^cmd``, and a CLI with a native
``--json`` mode decodes end to end (``^gh ... --json | from json``). For a nix
build, use the bundled ``nix`` module (a live dashboard build-tree pane), not
``^nix``.

Contract:

- ``await nu(code)`` returns a ``pl.DataFrame`` for tabular output: a
  table (list of records) maps directly, a list of scalars / a non-str
  scalar become a single ``value`` column, no output (``null``) an empty
  frame. A single record is a struct, not a table, so it comes back as a
  plain ``dict``: ``(await nu("... | complete"))['exit_code']`` just works
  instead of needing ``df.to_dicts()[0]`` first (issue #2390). A lone
  string -- an external's stdout, ``to text`` -- comes back as the plain
  ``str``: multiline text round-trips verbatim instead of hiding in a 1x1
  frame whose printed repr clips the cell (issue #2068).
- Multi-statement source: only the FINAL pipeline's output is the return
  value; every earlier pipeline's output is collected and printed into the
  job's stdout (script-style visibility), never silently dropped (issue
  #2391). ``^git show | to text; ^git status | to text`` returns the status
  and prints the show. To CAPTURE more than one pipeline's output, make
  separate calls (or build one final value, e.g. a record).
- ``await nu.value(code)`` is the escape hatch when you want the plain
  Python value (a scalar, a nested dict) instead of a frame.
- Values cross natively, not as JSON: dates arrive as UTC ``Datetime``
  columns, durations as ``Duration`` columns (microsecond resolution --
  Python's ``timedelta`` is the carrier, so a sub-microsecond remainder like
  ``1500ns`` truncates to the microsecond), filesize as ``int`` bytes,
  binary as ``bytes``.
- Externals run color-free by default: the engine env overrides
  ``NO_COLOR=1`` / ``CLICOLOR=0`` / ``CLICOLOR_FORCE=0`` / ``FORCE_COLOR=0``
  and never inherits ``GH_FORCE_TTY``, so a JSON-mode CLI pipes parseable
  bytes into ``from json`` even when the host process forces color. It
  also sets ``GH_PROMPT_DISABLED=1`` so gh errors out instead of trying
  to prompt into a captured pipe. A call
  that wants ANSI re-enables it with ``env={"NO_COLOR": "",
  "CLICOLOR_FORCE": "1"}`` (or ``with-env`` inside the pipeline).
- Each kernel session gets its OWN engine (stored in the session's
  namespace), so one agent's ``let``/``cd``/``def`` never leaks into or
  clobbers another session's pipelines.
- A failing pipeline raises :class:`NuError` whose message is nushell's own
  rendered diagnostic (span, label, and "did you mean" hints) -- read it,
  fix the pipeline, retry. ``exit`` raises too; it never ends this process.
- ``check=False`` is the grep escape hatch (subprocess.run semantics): a
  trailing external that exits non-zero stops raising and ``nu()`` returns
  :class:`NuResult` -- ``result`` is what the call would have returned
  anyway, built from the output the external DID produce, plus its
  ``exit_code`` -- so ``^ls | ^grep pattern`` with no match is an empty
  result with ``exit_code == 1``, not an exception. Only exit-status
  semantics change: parse errors, unknown commands, and runtime shell
  errors raise either way. A signal-terminated external reports the
  negative signal number, like subprocess.
- ``timeout=`` REQUESTS a stop the way ctrl-c would (nushell checks the
  flag between pipeline elements), raises ``TimeoutError``, and abandons
  the shared engine for a fresh one: a single stuck element (a hung
  ``http get``, an external that ignores the flag) may hold the old engine
  arbitrarily long, and abandoning it keeps later ``nu()`` calls from
  queueing behind the runaway. Persistent state is therefore LOST on a
  timeout. Cancelling the awaiting task interrupts the same way but keeps
  the engine (state survives); after cancelling a truly stuck pipeline,
  ``nu.reset()`` unwedges. An external the pipeline already spawned
  finishes on its own; run a genuinely long external as a background job
  you poll, or in its own ``nu.Engine()`` so a stuck one is isolated.
- Calls against the shared engine run one at a time (REPL state needs
  ordered evaluation); for parallel pipelines, construct separate
  ``nu.Engine()`` instances.
- ``nu.reset()`` discards the persistent state for a fresh engine.
"""

from __future__ import annotations

import asyncio
import contextlib
import html as _html
import inspect as _inspect
import os
import pathlib
import re
import time
from typing import TYPE_CHECKING, Literal, NamedTuple, overload

from ._nu import Engine, NuError

if TYPE_CHECKING:
    import polars as pl

__all__ = ["Engine", "NuError", "NuResult", "nu", "reset", "value"]

__version__ = "0.1.0"

# Job renaming mirrors sh: inside the kernel, `name=` labels the running job in
# the dashboard. Outside (plain `import nu` in a test), it is silently ignored.
try:
    from ix_notebook_mcp.runtime import _ix_current, _rename_current_job
    from ix_notebook_mcp.runtime import register_resource as _register_resource
except Exception:  # pragma: no cover - exercised only outside the kernel
    _ix_current = None
    _register_resource = None
    _rename_current_job = None

_engine: Engine | None = None
_RESOURCE_TAIL_CHARS = 120_000
_resource_counts: dict[str, int] = {}

# The per-session slot: engines live INSIDE the session's namespace dict, so
# an engine's lifetime is exactly its session's and one agent's `let`/`cd`
# never leaks into another session (module globals are shared across all
# session namespaces; the namespace dict itself is not).
_ENGINE_KEY = "__ix_nu_engine__"


def _session_slot() -> dict | None:
    """The current kernel job's session namespace, or None outside a job."""
    if _ix_current is None:
        return None
    job = _ix_current.get()
    ns = getattr(job, "_ns", None) if job is not None else None
    return ns if isinstance(ns, dict) else None


def _in_kernel_job() -> bool:
    return _ix_current is not None and _ix_current.get() is not None


def _next_resource_id(kind: str) -> str | None:
    if _ix_current is None:
        return None
    job = _ix_current.get()
    job_id = getattr(job, "id", None) if job is not None else None
    if not job_id:
        return None
    key = str(job_id)
    count = _resource_counts.get(key, 0) + 1
    _resource_counts[key] = count
    return f"{kind}-{key}-{count}"


def _title(code: str) -> str:
    clean = re.sub(r"\s+", " ", code).strip()
    if len(clean) > 72:
        clean = clean[:69] + "..."
    return f"nu: {clean or 'pipeline'}"


def _short_repr(value: object) -> str:
    rows = getattr(value, "to_dicts", None)
    if callable(rows):
        with contextlib.suppress(Exception):
            return repr(rows())
    text = repr(value)
    if len(text) > _RESOURCE_TAIL_CHARS:
        return "... truncated to tail\n" + text[-_RESOURCE_TAIL_CHARS:]
    return text


def _nu_resource_html(state: dict[str, object]) -> str:
    now = time.monotonic()
    ended = state.get("ended")
    end_time = ended if isinstance(ended, float) else now
    started = state.get("started")
    started_time = started if isinstance(started, float) else end_time
    duration = max(0.0, end_time - started_time)
    status = str(state.get("status") or "running")
    bad = status in {"failed", "timed out", "cancelled"}
    cls = "bad" if bad else "ok" if status == "done" else "running"
    code = _html.escape(str(state.get("code") or ""))
    cwd = state.get("cwd")
    cwd_html = f'<span class="cwd">{_html.escape(os.fspath(cwd))}</span>' if cwd else ""
    body_value = state.get("error") if bad else state.get("result")
    body = _html.escape("" if body_value is None else _short_repr(body_value))
    return (
        "<style>"
        "body{margin:0;background:#0f1117;color:#e5e7eb;font:12px ui-monospace,SFMono-Regular,Menlo,monospace}"
        ".wrap{padding:10px}.meta{display:flex;gap:8px;align-items:center;flex-wrap:wrap;margin-bottom:8px}"
        ".pill{border-radius:999px;padding:2px 8px;background:#334155}.ok{background:#064e3b;color:#a7f3d0}"
        ".bad{background:#7f1d1d;color:#fecaca}.running{background:#78350f;color:#fde68a}"
        ".cmd{color:#93c5fd;white-space:pre-wrap;word-break:break-word;margin-bottom:6px}"
        ".cwd{color:#9ca3af}pre{margin:0;white-space:pre-wrap;word-break:break-word}"
        "</style>"
        '<div class="wrap">'
        '<div class="meta">'
        f'<span class="pill {cls}">{_html.escape(status)}</span>'
        f"<span>{duration:.2f}s</span>{cwd_html}"
        "</div>"
        f'<div class="cmd">nu&gt; {code}</div>'
        f"<pre>{body}</pre>"
        "</div>"
    )


def _register_nu_resource(state: dict[str, object]) -> object | None:
    if _register_resource is None or not _in_kernel_job():
        return None
    rid = _next_resource_id("nu")
    if rid is None:
        return None
    return _register_resource(
        render=lambda: _nu_resource_html(state),
        id=rid,
        title=_title(str(state.get("code") or "")),
        kind="nu",
        alive=lambda: state.get("status") == "running",
    )


def _default_engine() -> Engine:
    """This session's engine, created on first use (module-global fallback
    outside a kernel job, e.g. tests and plain scripts)."""
    ns = _session_slot()
    if ns is not None:
        engine = ns.get(_ENGINE_KEY)
        if not isinstance(engine, Engine):
            engine = Engine()
            ns[_ENGINE_KEY] = engine
        return engine
    global _engine
    if _engine is None:
        _engine = Engine()
    return _engine


def _discard_engine(engine: Engine) -> None:
    """Drop ``engine`` from whichever slot holds it, so the next call gets a
    fresh one (the abandoned engine finishes and frees itself off-thread)."""
    global _engine
    if _engine is engine:
        _engine = None
    ns = _session_slot()
    if ns is not None and ns.get(_ENGINE_KEY) is engine:
        del ns[_ENGINE_KEY]


def reset() -> None:
    """Discard this session's persistent engine state (bindings, defs, and
    PWD)."""
    _discard_engine(_default_engine())


def _serialize_input(input: object) -> object:
    """``input=`` as plain Python the engine can convert to a nushell value.

    A polars frame becomes its rows (``to_dicts``), so nushell sees the same
    table shape it would produce itself; everything else passes through.
    """
    to_dicts = getattr(input, "to_dicts", None)
    if callable(to_dicts):  # a polars DataFrame, without importing polars here
        return to_dicts()
    return input


class NuResult(NamedTuple):
    """What ``nu(code, check=False)`` returns: the result plus the exit code.

    ``result`` is whatever the call would have returned anyway, built from
    the output the pipeline produced (a failing ``grep`` still hands over
    the lines it matched before exiting non-zero; a record stays a plain
    dict and a lone string stays plain text); ``exit_code`` is the trailing
    external's exit code, ``0`` when the pipeline ends in an internal
    command, negative-signal when the external was signal-terminated (the
    subprocess convention).
    """

    result: pl.DataFrame | dict[str, object] | str
    exit_code: int


def _require_dir(cwd: str | os.PathLike) -> None:
    """Reject an explicit ``cwd=`` that is not an existing directory. A bad
    cwd would be written into the persistent stack and wedge every later call
    (issue #1986's failure mode, self-inflicted); reject it at the boundary
    instead. Sync on purpose: one local ``stat``, and keeping the path method
    out of the ``async`` caller keeps it free of path methods (ASYNC240)."""
    if not pathlib.Path(cwd).is_dir():
        raise ValueError(f"cwd is not a directory: {os.fspath(cwd)!r}")


async def _run(
    code: str,
    *,
    input: object | None = None,
    cwd: str | os.PathLike | None = None,
    env: dict[str, str] | None = None,
    timeout: float | None = None,
    name: str | None = None,
    check: bool = True,
) -> tuple[object, int]:
    """Evaluate ``code`` on the shared engine; return ``(value, exit_code)``.

    With ``check=True`` a non-zero trailing external raises inside the engine,
    so the exit code of anything that returns is 0.
    """
    if cwd is not None:
        _require_dir(cwd)
    if name is not None and _rename_current_job is not None and (
        _ix_current is not None and _ix_current.get() is not None
    ):
        _rename_current_job(name)

    engine = _default_engine()
    loop = asyncio.get_running_loop()
    state: dict[str, object] = {
        "code": code,
        "cwd": os.fspath(cwd) if cwd is not None else None,
        "status": "running",
        "result": None,
        "error": None,
        "started": loop.time(),
        "ended": None,
    }
    resource = _register_nu_resource(state)
    coroutine, handle = engine.eval(
        code,
        input=_serialize_input(input) if input is not None else None,
        cwd=os.fspath(cwd) if cwd is not None else None,
        env=env,
        check=check,
    )
    try:
        decoded = await asyncio.wait_for(asyncio.ensure_future(coroutine), timeout)
    except TimeoutError:
        # The handle targets THIS eval only, so a timeout that fires while we
        # are still queued behind another pipeline can never interrupt it.
        handle.interrupt()
        # Abandon the engine, don't just interrupt it: the interrupt is only
        # honored between pipeline elements, so a stuck element could hold
        # this engine's lock indefinitely and every later nu() would queue
        # behind it. A fresh engine costs the persistent state (documented);
        # the abandoned one stops (and drops) whenever its element finishes.
        _discard_engine(engine)
        state["status"] = "timed out"
        state["error"] = f"nu pipeline timed out after {timeout}s (engine state discarded): {code}"
        state["ended"] = loop.time()
        close = getattr(resource, "close", None)
        if callable(close):
            close()
        raise TimeoutError(
            f"nu pipeline timed out after {timeout}s (engine state discarded): {code}"
        ) from None
    except asyncio.CancelledError:
        handle.interrupt()
        state["status"] = "cancelled"
        state["ended"] = loop.time()
        close = getattr(resource, "close", None)
        if callable(close):
            close()
        raise
    except Exception as exc:
        state["status"] = "failed"
        state["error"] = str(exc)
        state["ended"] = loop.time()
        close = getattr(resource, "close", None)
        if callable(close):
            close()
        raise
    else:
        # The engine resolves to an (intermediates, value, exit_code) triple:
        # each non-final pipeline's collected output, the final pipeline's
        # value, and the trailing external's exit code (always 0 under
        # check=True -- a non-zero trailing external raised inside the
        # engine).
        if not (isinstance(decoded, tuple) and len(decoded) == 3):
            raise TypeError(f"engine returned {type(decoded).__name__}, expected a triple")
        intermediates, value, exit_code = decoded
        if not isinstance(intermediates, list):
            raise TypeError(f"engine intermediates are {type(intermediates).__name__}")
        if not isinstance(exit_code, int):
            raise TypeError(f"engine exit code is {type(exit_code).__name__}")
        for item in intermediates:
            _print_intermediate(item)
        state["status"] = "done"
        state["result"] = value
        state["ended"] = loop.time()
        close = getattr(resource, "close", None)
        if callable(close):
            close()
        return value, exit_code


def _rows_frame(rows: list[dict]) -> pl.DataFrame:
    """Rows -> frame, surviving mixed-type columns.

    infer_schema_length=None scans every row so a column that starts null (or
    int) and later carries strings still gets one usable dtype; when even the
    full scan cannot unify (nushell data is legitimately heterogeneous, e.g.
    an `open`'d JSON with an int-then-string field), strict=False coerces to
    a supertype instead of breaking the always-a-DataFrame contract.
    """
    import polars as pl

    try:
        return pl.from_dicts(rows, infer_schema_length=None)
    except (TypeError, pl.exceptions.PolarsError):
        return pl.DataFrame(rows, infer_schema_length=None, strict=False)


def _print_intermediate(value: object) -> None:
    """One non-final pipeline's output, printed into the (captured) stdout.

    Script-style visibility for multi-statement source (issue #2391): the
    engine collects every intermediate pipeline's output and this prints it,
    so nothing is silently dropped while the FINAL pipeline's value stays the
    return value. Each item prints as the shape it would have been returned
    as: text verbatim (issue #2068's reasoning), a single record as the plain
    dict (issue #2390), anything else as a frame. ``None`` and the empty
    string (a statement with no output) stay silent -- printing them would
    only add blank lines between statements.
    """
    if value is None or value == "":
        return
    if isinstance(value, (str, dict)):
        print(value)
    else:
        print(_to_frame(value))


def _to_frame(decoded: object) -> pl.DataFrame:
    """Normalize a pipeline value into a ``pl.DataFrame`` (a lone ``str``
    or a single record never reaches here: ``nu()`` returns the ``str``
    verbatim (issue #2068) and the record as a plain ``dict`` (issue
    #2390))."""
    import polars as pl

    if decoded is None:
        return pl.DataFrame()
    if isinstance(decoded, list) and decoded and all(isinstance(r, dict) for r in decoded):
        return _rows_frame(decoded)
    if isinstance(decoded, list):
        try:
            series = pl.Series("value", decoded)
        except (TypeError, pl.exceptions.PolarsError):
            # Mixed scalars ([1, 2.5], [1, 'x']): supertype, not a crash.
            series = pl.Series("value", decoded, strict=False)
        return pl.DataFrame({"value": series})
    return pl.DataFrame({"value": [decoded]})


@overload
async def nu(
    code: str,
    *,
    input: object | None = ...,
    cwd: str | os.PathLike | None = ...,
    env: dict[str, str] | None = ...,
    timeout: float | None = ...,
    name: str | None = ...,
    check: Literal[True] = ...,
) -> pl.DataFrame | dict[str, object] | str: ...


@overload
async def nu(
    code: str,
    *,
    input: object | None = ...,
    cwd: str | os.PathLike | None = ...,
    env: dict[str, str] | None = ...,
    timeout: float | None = ...,
    name: str | None = ...,
    check: Literal[False],
) -> NuResult: ...


async def nu(
    code: str,
    *,
    input: object | None = None,
    cwd: str | os.PathLike | None = None,
    env: dict[str, str] | None = None,
    timeout: float | None = None,
    name: str | None = None,
    check: bool = True,
) -> pl.DataFrame | dict[str, object] | str | NuResult:
    """Run ``code`` as nushell source and return the result as a polars
    DataFrame for tabular output, a plain ``dict`` when the pipeline's value
    is a single record, or the plain ``str`` when it is a lone string.

    Multi-statement source is fine; the last pipeline's output is the result,
    every earlier pipeline's output prints into the job's stdout instead of
    being silently dropped (issue #2391), and ``let``/``def``/``cd`` persist
    to later calls (REPL semantics; the engine is per session, so another
    session's ``cd`` can never move this one's PWD). ``input`` pipes a value in as ``$in`` -- a polars DataFrame,
    list, dict, or scalar (datetimes must be tz-aware). ``cwd`` sets ``PWD``
    and persists like ``cd``; if the remembered directory no longer exists
    the call raises with the remedy instead of silently running elsewhere.
    ``env`` adds environment variables;
    ``timeout`` interrupts the evaluation and discards the engine state (see
    the module docstring); ``name`` labels the running job in the dashboard.

    Shape normalization: table -> frame; record -> plain ``dict`` (a struct,
    not a table: ``(await nu("do -i { ^cmd } | complete"))['exit_code']``
    reads directly, issue #2390); list of scalars / non-str scalar -> a
    single ``value`` column; a lone string -> the plain ``str`` (an
    external's stdout round-trips verbatim); no output -> empty frame. A
    failure raises :class:`NuError` carrying nushell's diagnostic.

    ``check=False`` (subprocess.run semantics) keeps a grep-style pipeline
    usable: a trailing external's non-zero exit stops raising and the call
    returns :class:`NuResult` -- ``result`` is whatever the call would have
    returned (the output the external did produce, a lone string staying
    text), plus ``exit_code`` -- so "no match" reads as an empty result with
    ``exit_code == 1``. Everything else still raises.
    """
    value, exit_code = await _run(
        code, input=input, cwd=cwd, env=env, timeout=timeout, name=name, check=check
    )
    result: pl.DataFrame | dict[str, object] | str
    if isinstance(value, str):
        # A lone string is TEXT (an external's stdout, `to text`), not a table:
        # hand it back verbatim. Framing it as a 1x1 DataFrame made every
        # `print()` of it write polars' width-clipped box repr into the
        # captured stdout, and the full text was unrecoverable afterwards
        # (issue #2068).
        result = value
    elif isinstance(value, dict):
        # A single record is a STRUCT, not a table: hand it back as the plain
        # dict. Framing it as a 1-row DataFrame forced every field read
        # through `df.to_dicts()[0]`, and the natural `d['exit_code']` /
        # `d.get('stderr')` on a `| complete` result failed with
        # "'DataFrame' object has no attribute 'get'" (issue #2390).
        result = value
    else:
        result = _to_frame(value)
    if check:
        return result
    return NuResult(result, exit_code)


async def value(
    code: str,
    *,
    input: object | None = None,
    cwd: str | os.PathLike | None = None,
    env: dict[str, str] | None = None,
    timeout: float | None = None,
    name: str | None = None,
) -> object:
    """Like :func:`nu`, but return the plain Python value un-framed.

    The escape hatch for a scalar (``await nu.value("sys host | get
    hostname")``) or a table/list you want as plain lists instead of a
    frame (a single record already arrives as a plain dict from :func:`nu`).
    """
    value, _ = await _run(code, input=input, cwd=cwd, env=env, timeout=timeout, name=name)
    return value


# Make the module itself callable (same pattern as the bundled `sh`), so the
# documented `await nu(...)` works bare while `nu.value` / `nu.NuError` stay
# reachable as attributes.
import sys as _sys
import types as _types

import functools as _functools


class _CallableModule(_types.ModuleType):
    @_functools.wraps(nu)
    def __call__(self, *args: object, **kwargs: object) -> object:
        return nu(*args, **kwargs)


_module = _sys.modules[__name__]
_module.__class__ = _CallableModule
# Publish the real callable signature so api() shows `code` and the kwargs
# instead of introspecting the bound __call__.
_module.__signature__ = _inspect.signature(nu)  # type: ignore[attr-defined]
