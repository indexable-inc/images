"""An embedded nushell engine; every pipeline result is a polars DataFrame.

Bundled like ``sh``/``view`` so every session can ``await nu(...)`` with no
setup. The point: ``sh`` + ``.json()/.df()`` only speaks structured data when
the CLI has a JSON mode; everything else decays into jq/awk/sed text
scraping. Nushell's pipelines are structured end to end (``ls``, ``ps``,
``open``, ``from csv|toml|yaml``, ``where``, ``group-by``), so ``nu()`` is
the bridge from "shell pipeline" to "typed frame" for any data-shaped
command::

    df = await nu("ls | where size > 1kb | sort-by size")
    df = await nu("ps | where cpu > 5 | sort-by cpu")
    df = await nu("open Cargo.toml | get package")          # record -> 1-row frame
    df = await nu("http get https://api.github.com/repos/nushell/nushell")

This is not a subprocess: the engine (PyO3 bindings over nu-engine) lives in
this process and its state is persistent, like a REPL. A ``let``, a ``def``,
or a ``cd`` in one call is visible to the next::

    await nu("let prs = (http get $url)")
    await nu("$prs | where author.login == 'andrewgazelka'")

A DataFrame (or list/dict/scalar) can be piped THROUGH a pipeline: pass
``input=`` and the code sees it as its pipeline input (``$in``)::

    df = await nu("where size > 1kb | sort-by size", input=df)

Prefer ``nu()`` over ``sh`` whenever you want a command's *data*. ``sh``
remains the tool for side-effectful commands (``git``/``nix``/``gh`` writes)
and raw logs, and ``sh(...).df()`` for CLIs with a native ``--json`` mode.

Contract:

- ``await nu(code)`` returns a ``pl.DataFrame``, always: a table maps
  directly, a record becomes one row, a list of scalars / a scalar become a
  single ``value`` column, no output (``null``) an empty frame.
- ``await nu.value(code)`` is the escape hatch when you want the plain
  Python value (a scalar, a nested dict) instead of a frame.
- Values cross natively, not as JSON: dates arrive as UTC ``Datetime``
  columns, durations as ``Duration`` columns, filesize as ``int`` bytes,
  binary as ``bytes``.
- A failing pipeline raises :class:`NuError` whose message is nushell's own
  rendered diagnostic (span, label, and "did you mean" hints) -- read it,
  fix the pipeline, retry. ``exit`` raises too; it never ends this process.
- ``timeout=`` REQUESTS a stop the way ctrl-c would (nushell checks the
  flag between pipeline elements), raises ``TimeoutError``, and abandons
  the shared engine for a fresh one: a single stuck element (a hung
  ``http get``, an external that ignores the flag) may hold the old engine
  arbitrarily long, and abandoning it keeps later ``nu()`` calls from
  queueing behind the runaway. Persistent state is therefore LOST on a
  timeout. Cancelling the awaiting task interrupts the same way but keeps
  the engine (state survives); after cancelling a truly stuck pipeline,
  ``nu.reset()`` unwedges. An external the pipeline already spawned
  finishes on its own; for long externals prefer ``sh``, which group-kills
  on timeout.
- Calls against the shared engine run one at a time (REPL state needs
  ordered evaluation); for parallel pipelines, construct separate
  ``nu.Engine()`` instances.
- ``nu.reset()`` discards the persistent state for a fresh engine.
"""

from __future__ import annotations

import asyncio
import inspect as _inspect
import os
from typing import TYPE_CHECKING

from ._nu import Engine, NuError

if TYPE_CHECKING:
    import polars as pl

__all__ = ["Engine", "NuError", "nu", "reset", "value"]

__version__ = "0.1.0"

# Job renaming mirrors sh: inside the kernel, `name=` labels the running job in
# the dashboard. Outside (plain `import nu` in a test), it is silently ignored.
try:
    from ix_notebook_mcp.runtime import _ix_current, _rename_current_job
except Exception:  # pragma: no cover - exercised only outside the kernel
    _ix_current = None
    _rename_current_job = None

_engine: Engine | None = None


def _default_engine() -> Engine:
    """The shared engine behind module-level calls, created on first use."""
    global _engine
    if _engine is None:
        _engine = Engine()
    return _engine


def reset() -> None:
    """Discard the persistent engine state (bindings, defs, cwd, env)."""
    global _engine
    _engine = None


def _serialize_input(input: object) -> object:
    """``input=`` as plain Python the engine can convert to a nushell value.

    A polars frame becomes its rows (``to_dicts``), so nushell sees the same
    table shape it would produce itself; everything else passes through.
    """
    to_dicts = getattr(input, "to_dicts", None)
    if callable(to_dicts):  # a polars DataFrame, without importing polars here
        return to_dicts()
    return input


async def _run(
    code: str,
    *,
    input: object | None = None,
    cwd: str | os.PathLike | None = None,
    env: dict[str, str] | None = None,
    timeout: float | None = None,
    name: str | None = None,
) -> object:
    """Evaluate ``code`` on the shared engine; return the plain Python value."""
    if name is not None and _rename_current_job is not None and (
        _ix_current is not None and _ix_current.get() is not None
    ):
        _rename_current_job(name)

    engine = _default_engine()
    coroutine = engine.eval(
        code,
        input=_serialize_input(input) if input is not None else None,
        cwd=os.fspath(cwd) if cwd is not None else None,
        env=env,
    )
    try:
        return await asyncio.wait_for(asyncio.ensure_future(coroutine), timeout)
    except TimeoutError:
        engine.interrupt()
        # Abandon the engine, don't just interrupt it: the interrupt is only
        # honored between pipeline elements, so a stuck element could hold
        # this engine's lock indefinitely and every later nu() would queue
        # behind it. A fresh engine costs the persistent state (documented);
        # the abandoned one stops (and drops) whenever its element finishes.
        if engine is _engine:
            reset()
        raise TimeoutError(
            f"nu pipeline timed out after {timeout}s (engine state discarded): {code}"
        ) from None
    except asyncio.CancelledError:
        engine.interrupt()
        raise


def _to_frame(decoded: object) -> pl.DataFrame:
    """Normalize any pipeline value into a ``pl.DataFrame``."""
    import polars as pl

    if decoded is None:
        return pl.DataFrame()
    if isinstance(decoded, list) and decoded and all(isinstance(r, dict) for r in decoded):
        # infer_schema_length=None: scan every row, so a column that starts
        # null (or int) and later carries strings still gets one usable dtype.
        return pl.from_dicts(decoded, infer_schema_length=None)
    if isinstance(decoded, dict):
        return pl.from_dicts([decoded], infer_schema_length=None)
    if isinstance(decoded, list):
        return pl.DataFrame({"value": pl.Series("value", decoded)})
    return pl.DataFrame({"value": [decoded]})


async def nu(
    code: str,
    *,
    input: object | None = None,
    cwd: str | os.PathLike | None = None,
    env: dict[str, str] | None = None,
    timeout: float | None = None,
    name: str | None = None,
) -> pl.DataFrame:
    """Run ``code`` as nushell source and return the result as a polars
    DataFrame.

    Multi-statement source is fine; the last pipeline's output is the result,
    and ``let``/``def``/``cd`` persist to later calls (REPL semantics).
    ``input`` pipes a value in as ``$in`` -- a polars DataFrame, list, dict,
    or scalar (datetimes must be tz-aware). ``cwd`` sets ``PWD``
    (persistently, like ``cd``); ``env`` adds environment variables;
    ``timeout`` interrupts the evaluation and discards the engine state (see
    the module docstring); ``name`` labels the running job in the dashboard.

    Shape normalization: table -> frame; record -> 1-row frame; list of
    scalars / scalar -> a single ``value`` column; no output -> empty frame.
    A failure raises :class:`NuError` carrying nushell's diagnostic.
    """
    decoded = await _run(code, input=input, cwd=cwd, env=env, timeout=timeout, name=name)
    return _to_frame(decoded)


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
    return await _run(code, input=input, cwd=cwd, env=env, timeout=timeout, name=name)


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
