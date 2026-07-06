"""An embedded nushell engine; every pipeline result is a polars DataFrame.

Bundled like ``view`` so every session can ``await nu(...)`` with no setup.
``nu`` is the ONE shell-out path (the old ``sh``/``zsh`` are retired). Nushell's
pipelines are structured end to end (``ls``, ``ps``, ``open``,
``from csv|toml|yaml``, ``where``, ``group-by``), so ``nu()`` is the bridge from
"shell pipeline" to "typed frame" for any data-shaped command, and an external
binary runs with ``^cmd``::

    df = await nu("ls | where size > 1kb | sort-by size")
    df = await nu("ps | where cpu > 5 | sort-by cpu")
    df = await nu("open Cargo.toml | get package")          # record -> 1-row frame
    df = await nu("http get https://api.github.com/repos/nushell/nushell")
    text = await nu("^git status --short")                  # external binary via ^ (stdout str)
    df = await nu("^gh pr list --json number,title | from json")  # JSON-mode CLI

This is not a subprocess: the engine (PyO3 bindings over nu-engine) lives in
this process and its state is persistent, like a REPL. A ``let`` or a ``def``
in one call is visible to the next (``PWD`` is the exception: it re-syncs to
the process cwd each call, so a ``cd`` never outlives its call)::

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

- ``await nu(code)`` returns a ``pl.DataFrame`` for structured output: a
  table maps directly, a record becomes one row, a list of scalars / a
  non-str scalar become a single ``value`` column, no output (``null``) an
  empty frame. A lone string -- an external's stdout, ``to text`` -- comes
  back as the plain ``str``: multiline text round-trips verbatim instead of
  hiding in a 1x1 frame whose printed repr clips the cell (issue #2068).
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
  bytes into ``from json`` even when the host process forces color. A call
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
    """Discard this session's persistent engine state (bindings and defs;
    PWD is per-call, synced from the process cwd)."""
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
    the lines it matched before exiting non-zero; a lone string stays plain
    text); ``exit_code`` is the trailing external's exit code, ``0`` when
    the pipeline ends in an internal command, negative-signal when the
    external was signal-terminated (the subprocess convention).
    """

    result: pl.DataFrame | str
    exit_code: int


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
    if cwd is None:
        cwd = pathlib.Path.cwd()
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
        # check=False resolves to a (value, exit_code) pair; check=True keeps
        # the engine's historical value-only shape (exit code 0 by survival --
        # a non-zero trailing external raised inside the engine).
        if check:
            value, exit_code = decoded, 0
        else:
            if not (isinstance(decoded, tuple) and len(decoded) == 2):
                raise TypeError(f"engine returned {type(decoded).__name__} for check=False")
            value, exit_code = decoded
            if not isinstance(exit_code, int):
                raise TypeError(f"engine exit code is {type(exit_code).__name__}")
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


def _to_frame(decoded: object) -> pl.DataFrame:
    """Normalize a pipeline value into a ``pl.DataFrame`` (a lone ``str``
    never reaches here: ``nu()`` returns it verbatim, issue #2068)."""
    import polars as pl

    if decoded is None:
        return pl.DataFrame()
    if isinstance(decoded, list) and decoded and all(isinstance(r, dict) for r in decoded):
        return _rows_frame(decoded)
    if isinstance(decoded, dict):
        return _rows_frame([decoded])
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
) -> pl.DataFrame | str: ...


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
) -> pl.DataFrame | str | NuResult:
    """Run ``code`` as nushell source and return the result as a polars
    DataFrame, or as the plain ``str`` when the pipeline's value is a lone
    string.

    Multi-statement source is fine; the last pipeline's output is the result,
    and ``let``/``def`` persist to later calls (REPL semantics). ``PWD`` does
    not: each call re-syncs it to the process cwd, so a ``cd`` never outlives
    its call. ``input`` pipes a value in as ``$in`` -- a polars DataFrame,
    list, dict, or scalar (datetimes must be tz-aware). ``cwd`` sets ``PWD``
    for this call only; ``env`` adds environment variables;
    ``timeout`` interrupts the evaluation and discards the engine state (see
    the module docstring); ``name`` labels the running job in the dashboard.

    Shape normalization: table -> frame; record -> 1-row frame; list of
    scalars / non-str scalar -> a single ``value`` column; a lone string ->
    the plain ``str`` (an external's stdout round-trips verbatim); no output
    -> empty frame. A failure raises :class:`NuError` carrying nushell's
    diagnostic.

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
    if isinstance(value, str):
        # A lone string is TEXT (an external's stdout, `to text`), not a table:
        # hand it back verbatim. Framing it as a 1x1 DataFrame made every
        # `print()` of it write polars' width-clipped box repr into the
        # captured stdout, and the full text was unrecoverable afterwards
        # (issue #2068).
        result: pl.DataFrame | str = value
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
    hostname")``) or a nested structure you want as plain dicts/lists.
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
