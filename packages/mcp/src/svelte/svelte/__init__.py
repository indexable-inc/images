"""Svelte 5 components as live interactive dashboard resources.

Author real components instead of hand-rolled HTML/JS strings::

    import svelte

    res = await svelte.component(
        "Board.svelte",              # a .svelte file, or inline component source
        id="checkers", title="checkers",
        state=lambda: game_state,    # dict (or callable returning one)
        actions={"move": on_move},   # same contract as register_resource
    )

The component imports the virtual ``ix`` module::

    <script>
      import { data, act, replies, error } from 'ix';
    </script>
    <h1>count: {$data.count}</h1>
    <button onclick={() => act('bump', { by: 1 })}>+1</button>

``$data`` is the resource state: seeded from ``state`` at render, replaced by
every action handler's returned dict (the ``action_result`` event the page
already receives). ``act`` queues a payload for the named in-kernel handler,
``replies`` collects agent ``reply`` messages, ``error`` is the last action
error. One renderer, kernel state as the single source of truth.

Compilation shells out to the ``svelte-bundle`` CLI (esbuild + esbuild-svelte,
``IX_SVELTE_BUNDLE_BIN`` on the wrapper): one self-contained IIFE bundle with
injected CSS, so the sandboxed opaque-origin iframe needs no network and no
build step at view time.
"""

from __future__ import annotations

import asyncio
import json
import os
import re
import shutil
import tempfile
from collections.abc import Callable, Mapping
from pathlib import Path
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from ix_notebook_mcp.runtime import Resource

__all__ = ["SvelteError", "bundle", "component"]


class SvelteError(RuntimeError):
    """svelte-bundle failed (compile error, or the CLI is not wired up)."""


def _bin() -> str:
    exe = os.environ.get("IX_SVELTE_BUNDLE_BIN") or shutil.which("svelte-bundle")
    if not exe:
        raise SvelteError(
            "svelte-bundle CLI not found: IX_SVELTE_BUNDLE_BIN is unset and no "
            "`svelte-bundle` on PATH (it is set on the ix-mcp wrapper)"
        )
    return exe


def _entry_path(source: str | os.PathLike[str]) -> tuple[Path, Path | None]:
    """An existing ``.svelte`` file passes through; anything that reads as
    markup is written into a fresh private ``mkdtemp`` dir (mode 0700,
    unguessable: a predictable shared-/tmp path would be a symlink/TOCTOU
    vector and a concurrent-write race). Returns ``(entry, tmpdir-to-delete)``.
    Inline components cannot use relative imports; put multi-file components
    on disk."""
    p = Path(source)
    try:
        if p.is_file():
            return p, None
    except OSError:  # a long inline source is not a valid path
        pass
    text = str(source)
    if "<" not in text and "{" not in text:
        raise SvelteError(f"no such .svelte file: {source!r}")
    tmpdir = Path(tempfile.mkdtemp(prefix="ix-svelte-"))
    entry = tmpdir / "Component.svelte"
    entry.write_text(text)
    return entry, tmpdir


async def bundle(source: str | os.PathLike[str], *, minify: bool = False) -> str:
    """Compile a Svelte 5 component to one self-contained IIFE bundle (JS text)."""
    entry, tmpdir = _entry_path(source)
    argv = [_bin(), str(entry), *(["--minify"] if minify else [])]
    try:
        proc = await asyncio.create_subprocess_exec(
            *argv,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        out, err = await proc.communicate()
    finally:
        if tmpdir is not None:
            shutil.rmtree(tmpdir, ignore_errors=True)
    if proc.returncode != 0:
        raise SvelteError(f"svelte-bundle failed for {entry}:\n{err.decode(errors='replace')}")
    return out.decode()


def _inline_js_safe(js: str) -> str:
    # Two HTML script-data sequences can escape an inline <script>: `</script`
    # (case-insensitive) closes it, and `<!--` enters the escaped state where a
    # later `<script` swallows following tags. Both rewrites are byte-identical
    # inside JS strings; outside a string either sequence is (at worst) a loud
    # syntax error instead of silent tag breakout. The bundle is author-trusted
    # (same trust domain as kernel code), so this guards accidents, not attacks;
    # untrusted data belongs in the state seed, which `_seed_json` fully escapes.
    return re.sub(r"(?i)<(/script)", r"<\\\1", js).replace("<!--", "<\\!--")


def _seed_json(value: object) -> str:
    # In JSON output `<` only ever occurs inside string literals, so escaping
    # it as `\u003c` is always valid and blocks every script-data breakout
    # (`</script`, `<!--`, `<script`) regardless of case or context. State may
    # carry untrusted bytes (handlers fold in external data), so this is the
    # real injection boundary.
    return json.dumps(value, default=str).replace("<", "\\u003c")


async def component(
    source: str | os.PathLike[str],
    *,
    id: str,
    title: str | None = None,
    state: Mapping[str, Any] | Callable[[], Any] | None = None,
    actions: Mapping[str, Any] | None = None,
    minify: bool = False,
) -> Resource:
    """Compile ``source`` and register it as a live interactive resource.

    ``state`` seeds ``window.__IX_STATE__`` (the ``data`` store's initial
    value) on every render: pass the dict your action handlers return, or a
    callable re-read on each pane refresh. Re-calling with the same ``id``
    recompiles and replaces the resource (the edit loop).
    """
    js = _inline_js_safe(await bundle(source, minify=minify))
    state_fn = state if callable(state) else (lambda: state or {})

    async def _render() -> str:
        s = state_fn()
        if asyncio.iscoroutine(s):
            s = await s
        return f"<script>window.__IX_STATE__ = {_seed_json(s)};</script><script>{js}</script>"

    from ix_notebook_mcp.runtime import register_resource

    return register_resource(render=_render, id=id, title=title or id, actions=actions)
