"""Parse a ``nix --log-format internal-json`` stream into polars + a live DAG.

Bundled like ``view``/``fff``/``search`` so every session can ``import nix`` with
no setup. Nix's ``internal-json`` logger emits one ``@nix {...}`` line per event
(an activity starting/stopping, a progress tick, a build-log line, an error).
Reading that by hand is miserable; this turns it into the two things an agent and
a human actually want:

* :attr:`NixLog.events` -- the **durable log**: one ``polars.DataFrame`` row per
  ``@nix`` line, faithfully. This is "the whole internal-json as a frame": filter
  it, group it, join it, keep it.
* :attr:`NixLog.activities` -- a **derived view**: one row per activity (the
  nodes of the build DAG), folded from the stream with live ``status`` /
  ``done`` / ``expected`` / ``phase`` / ``last_log`` / ``drv``. This is what
  answers "what is building right now?".

The transport (the raw lines), the durable log (``events``), and the per-query
view (``activities``) stay distinct: each line is recorded once and every view is
derived from it.

Two ways in:

* :func:`parse` -- pure, synchronous, over text or an iterable of lines. No
  subprocess, so it is trivially testable and works on a captured log. Returns a
  :class:`NixLog` (``.events`` / ``.activities`` polars frames, ``.tree()``).
* :func:`run` / :func:`build` -- async: the *live* path. Rather than re-parsing
  internal-json in Python, they spawn the Rust ``nix-web-monitor`` emitter
  (``--emit ndjson``), the single owner of that parsing, which folds the stream
  into a build tree and streams a compact ``BuildView`` per state change. Each
  line updates a :class:`BuildRun`, and (in a kernel session) a live dashboard
  pane rendered by the native ``nix-build`` renderer shows the tree grow and
  self-close when the build finishes. The model gets the :class:`BuildRun` back
  (``.builds`` is a polars frame, ``.ok`` / ``.errors`` / ``.tree()``).

Run it as a background job (``await nix.build(".#foo")`` runs past the budget and
backgrounds); next turn ``await jobs['..']`` yields the finished :class:`BuildRun`,
while the dashboard pane shows the tree growing live as it builds.
"""

from __future__ import annotations

import asyncio
import contextlib
import html as _html
import json as _json
import os
import re
import time
from collections.abc import Iterable
from typing import TYPE_CHECKING, Any

import polars as pl

if TYPE_CHECKING:
    from ix_notebook_mcp.runtime import Resource

__all__ = [
    "ACTIVITY_TYPES",
    "RESULT_TYPES",
    "BuildRun",
    "NixLog",
    "attrs",
    "build",
    "eval",
    "parse",
    "run",
]

__version__ = "0.1.0"

# Activity types (`start`/`stop` actions), from Nix's `Logger::Activity` enum.
ACTIVITY_TYPES = {
    0: "unknown",
    100: "copyPath",
    101: "fileTransfer",
    102: "realise",
    103: "copyPaths",
    104: "builds",
    105: "build",
    106: "optimiseStore",
    107: "verifyPaths",
    108: "substitute",
    109: "queryPathInfo",
    110: "postBuildHook",
    111: "buildWaiting",
}

# Result types (`result` action), from Nix's `Logger::ResultType` enum.
RESULT_TYPES = {
    100: "fileLinked",
    101: "buildLogLine",
    102: "untrustedPath",
    103: "corruptedPath",
    104: "setPhase",
    105: "progress",
    106: "setExpected",
    107: "postBuildLogLine",
    108: "fetchStatus",
}

_ANSI = re.compile(r"\x1b\[[0-9;]*m")

# Stable schema so `.events` is a well-typed frame even with zero rows.
_EVENT_SCHEMA = {
    "seq": pl.Int64,
    "action": pl.Utf8,
    "id": pl.Int64,
    "parent": pl.Int64,
    "type": pl.Int64,
    "kind": pl.Utf8,
    "level": pl.Int64,
    "text": pl.Utf8,
    "msg": pl.Utf8,
    "fields": pl.Utf8,
}

_ACTIVITY_SCHEMA = {
    "id": pl.Int64,
    "parent": pl.Int64,
    "kind": pl.Utf8,
    "status": pl.Utf8,
    "done": pl.Int64,
    "expected": pl.Int64,
    "phase": pl.Utf8,
    "last_log": pl.Utf8,
    "drv": pl.Utf8,
    "depth": pl.Int64,
    "start_seq": pl.Int64,
    "end_seq": pl.Int64,
}


def _strip(s: object) -> str:
    return _ANSI.sub("", s if isinstance(s, str) else "" if s is None else str(s))


def _as_int(v: object) -> int:
    if isinstance(v, (int, str, bytes)):
        try:
            return int(v)
        except (TypeError, ValueError):
            return 0
    return 0


class NixLog:
    """A growing record of one ``nix`` invocation's ``internal-json`` stream.

    Feed it raw lines with :meth:`feed` (``run`` does this for you); read
    :attr:`events` / :attr:`activities` (polars frames) and :meth:`tree` at any
    point, including mid-build. The object is mutated in place, so a handle to it
    is a live view of an in-flight build.
    """

    def __init__(self, *, label: str | None = None) -> None:
        self.label = label
        self.lines: list[str] = []
        self._acts: dict[int, dict[str, Any]] = {}
        self._order: list[int] = []
        self.error: str | None = None
        self.done = False
        self.returncode: int | None = None
        self.started = time.time()

    @property
    def ok(self) -> bool:
        """True once the build has finished successfully (done, exit 0, no error),
        mirroring ``sh()``'s ``Output.ok`` so "await a build, branch on success"
        reads the same across both. False while still running, so it never reads
        like a missing attribute the way ``getattr(log, "ok", None)`` -> None did."""
        return self.done and self.returncode == 0 and self.error is None

    def feed(self, line: str) -> None:
        """Consume one line of nix output (``@nix {...}`` JSON or plain text)."""
        line = line.rstrip("\n")
        self.lines.append(line)
        if not line.startswith("@nix "):
            return
        try:
            # strict=False: nix can emit raw control chars (ANSI) inside a string;
            # tolerate them rather than silently drop the line.
            o = _json.loads(line[5:], strict=False)
        except _json.JSONDecodeError:
            return
        action = o.get("action")
        aid = o.get("id")
        if action == "start":
            if aid not in self._acts:
                self._order.append(aid)
            fields: list[Any] = o.get("fields") or []
            self._acts[aid] = {
                "id": aid,
                "parent": o.get("parent") or 0,
                "kind": ACTIVITY_TYPES.get(o.get("type"), "?"),
                "status": "running",
                "done": 0,
                "expected": 0,
                "phase": None,
                "last_log": None,
                # actBuild (105) fields = [drvPath, machine, round, totalRounds].
                "drv": fields[0] if o.get("type") == 105 and fields else None,
                "start_seq": len(self.lines) - 1,
                "end_seq": None,
                "text": _strip(o.get("text")),
            }
        elif action == "stop":
            act = self._acts.get(aid)
            if act is not None:
                act["status"] = "done"
                act["end_seq"] = len(self.lines) - 1
        elif action == "result":
            act = self._acts.get(aid)
            if act is None:
                return
            rtype = o.get("type")
            fields = o.get("fields") or []
            if rtype == 105 and len(fields) >= 2:  # progress: [done, expected, ...]
                # Coerce defensively: the Int64 schema would crash `.activities`
                # on a non-numeric field (nix always sends ints, but never trust).
                act["done"], act["expected"] = _as_int(fields[0]), _as_int(fields[1])
            elif rtype == 104 and fields:  # setPhase: [phase]
                act["phase"] = _strip(fields[0])
            elif rtype in (101, 107) and fields:  # build / post-build log line
                act["last_log"] = _strip(fields[0])
        elif action == "msg":
            msg = _strip(o.get("msg") or o.get("raw_msg"))
            # Nix Verbosity: lvlError=0, lvlWarn=1, ... Capture only true errors
            # (level 0); a warning or an info line that merely contains "error:"
            # is not the build's failure. First error wins (root cause prints first).
            if msg and o.get("level") == 0:
                self.error = self.error or msg

    # -- frames ---------------------------------------------------------------

    @property
    def events(self) -> pl.DataFrame:
        """One row per ``@nix`` line: the durable, faithful log."""
        rows = []
        for seq, line in enumerate(self.lines):
            if not line.startswith("@nix "):
                continue
            try:
                o = _json.loads(line[5:], strict=False)
            except _json.JSONDecodeError:
                continue
            action = o.get("action")
            typ = o.get("type")
            rows.append(
                {
                    "seq": seq,
                    "action": action,
                    "id": o.get("id"),
                    "parent": o.get("parent"),
                    "type": typ,
                    "kind": (
                        ACTIVITY_TYPES.get(typ)
                        if action == "start"
                        else RESULT_TYPES.get(typ)
                        if action == "result"
                        else None
                    ),
                    "level": o.get("level"),
                    "text": _strip(o.get("text")),
                    "msg": _strip(o.get("msg") or o.get("raw_msg")),
                    "fields": (
                        _json.dumps(o["fields"]) if o.get("fields") is not None else None
                    ),
                }
            )
        return pl.DataFrame(rows, schema=_EVENT_SCHEMA)

    @property
    def activities(self) -> pl.DataFrame:
        """One row per activity (DAG node), folded from the stream."""
        if not self._order:
            return pl.DataFrame(schema=_ACTIVITY_SCHEMA)
        depth = self._depths()
        rows = [
            {k: a.get(k) for k in _ACTIVITY_SCHEMA if k != "depth"} | {"depth": depth[a["id"]]}
            for a in (self._acts[i] for i in self._order)
        ]
        return pl.DataFrame(rows, schema=_ACTIVITY_SCHEMA)

    def _depths(self) -> dict[int, int]:
        depth: dict[int, int] = {}

        def d(i: int, seen: frozenset[int] = frozenset()) -> int:
            if i in depth:
                return depth[i]
            act = self._acts.get(i)
            parent = act["parent"] if act is not None else 0
            # Guard a malformed parent cycle (self-parent or a→b→a) so a forward
            # reference can't infinite-recurse the live render path.
            if act is None or parent == 0 or parent in seen or parent == i:
                depth[i] = 0
            else:
                depth[i] = d(parent, seen | {i}) + 1
            return depth[i]

        return {i: d(i) for i in self._order}

    # -- rendering ------------------------------------------------------------

    def tree(self) -> str:
        """The activity DAG as an indented text tree (``✓`` done, ``▶`` running)."""
        by_parent: dict[int, list[dict[str, Any]]] = {}
        for i in self._order:
            r = self._acts[i]
            by_parent.setdefault(r["parent"], []).append(r)
        out: list[str] = []

        def walk(pid: int, d: int) -> None:
            for r in by_parent.get(pid, []):
                mark = "✓" if r["status"] == "done" else "▶"
                prog = f" {r['done']}/{r['expected']}" if r["expected"] else ""
                tail = f"  ← {r['last_log']}" if r["last_log"] else ""
                label = r["text"] or r["kind"]
                out.append(f"{'  ' * d}{mark} [{r['kind']}]{prog} {label[:72]}{tail}")
                walk(r["id"], d + 1)

        walk(0, 0)
        return "\n".join(out) or "(starting…)"

    def _summary(self) -> str:
        running = sum(1 for a in self._acts.values() if a["status"] == "running")
        done = sum(1 for a in self._acts.values() if a["status"] == "done")
        state = "error" if self.error else "done" if self.done else "building…"
        return f"{state} · {done} done · {running} running · {time.time() - self.started:.0f}s"

    def resource_html(self) -> str:
        """Current HTML for a live dashboard Resource (see :func:`run`)."""
        head = f"<b>nix</b> {_html.escape(self.label or '')} · {_html.escape(self._summary())}"
        err = (
            f"<div style='color:#e5534b;white-space:pre-wrap'>{_html.escape(self.error)}</div>"
            if self.error
            else ""
        )
        return (
            "<div style='font:12px ui-monospace,monospace;line-height:1.45'>"
            f"{head}{err}<pre style='margin:.4em 0 0'>{_html.escape(self.tree())}</pre></div>"
        )

    _repr_html_ = resource_html

    def __repr__(self) -> str:
        label = f" {self.label}" if self.label else ""
        return f"<NixLog{label} [{self._summary()}] {len(self._acts)} activities>"


def parse(source: str | Iterable[str]) -> NixLog:
    """Parse a captured ``internal-json`` stream (text or lines) into a NixLog."""
    log = NixLog()
    lines = source.splitlines() if isinstance(source, str) else source
    for line in lines:
        log.feed(line)
    log.done = True
    return log


# The BuildView renderer registered in the dashboard frontend (see
# packages/dashboard/dashboard-core/site/src/lib/renderers.ts). A live `data`
# resource with this renderer draws the build tree natively.
_NIX_BUILD_RENDERER = "nix-build"


def _nix_web_monitor_bin() -> str:
    """The `nix-web-monitor` binary the emitter runs. The mcp wrapper bakes its
    path onto the env (``IX_NIX_WEB_MONITOR_BIN``); outside that wrapper (a bare
    interpreter, a test) fall back to the name on PATH, so a dev shell with the
    package installed still works."""
    return os.environ.get("IX_NIX_WEB_MONITOR_BIN") or "nix-web-monitor"


class BuildRun:
    """A live record of one ``nix`` invocation, driven by the Rust
    ``nix-web-monitor`` emitter (the single owner of internal-json parsing).

    Where :class:`NixLog` parses a *captured* stream in Python, a ``BuildRun`` is
    the *live* path: :func:`run` spawns ``nix-web-monitor --emit ndjson``, which
    streams a compact ``BuildView`` (one JSON object per line) as the build
    progresses, and each line replaces :attr:`view` here. A handle to it is a
    live view of the in-flight build; in a kernel it also shows up as a
    self-closing dashboard pane rendered by the native ``nix-build`` renderer.
    """

    def __init__(self, *, label: str | None = None) -> None:
        self.label = label
        # The latest BuildView the emitter sent (its camelCase JSON shape, owned
        # by the parser crate). None until the first line lands.
        self.view: dict[str, Any] | None = None
        self.done = False
        self.returncode: int | None = None
        self.started = time.time()
        # Set when the emitter binary could not be spawned at all (so there is no
        # process and no BuildView), surfaced through :attr:`error`.
        self._spawn_error: str | None = None

    def feed(self, line: str) -> None:
        """Consume one NDJSON line from the emitter, replacing :attr:`view`.

        A blank line (or one that is not a JSON object with the BuildView shape)
        is ignored, so a stray diagnostic on the channel never corrupts state.
        """
        line = line.strip()
        if not line:
            return
        try:
            obj = _json.loads(line)
        except _json.JSONDecodeError:
            return
        if isinstance(obj, dict) and "builds" in obj and "counts" in obj:
            self.view = obj

    @property
    def error(self) -> str | None:
        """The build's first error message, or a synthetic one for a non-zero
        exit that reported none. Mirrors :attr:`NixLog.error`."""
        if self._spawn_error is not None:
            return self._spawn_error
        errors: list[Any] = (self.view or {}).get("errors") or []
        if errors:
            return str(errors[0])
        if self.done and self.returncode not in (0, None):
            return f"nix exited with code {self.returncode}"
        return None

    @property
    def ok(self) -> bool:
        """True once the build finished successfully (done, exit 0, no error),
        matching :attr:`NixLog.ok` and ``sh()``'s ``Output.ok`` so callers can
        branch on success the same way across all three."""
        return self.done and self.returncode == 0 and not (self.view or {}).get("errors")

    @property
    def builds(self) -> pl.DataFrame:
        """One row per derivation (name, status, phase, log count), from the
        latest ``BuildView``. A well-typed empty frame before the first line."""
        rows: list[dict[str, Any]] = (self.view or {}).get("builds") or []
        schema = {
            "derivation": pl.Utf8,
            "name": pl.Utf8,
            "status": pl.Utf8,
            "phase": pl.Utf8,
            "host": pl.Utf8,
            "logCount": pl.Int64,
            "contentAddressed": pl.Boolean,
        }
        return pl.DataFrame([{k: r.get(k) for k in schema} for r in rows], schema=schema)

    @property
    def errors(self) -> list[str]:
        """The error messages in the latest ``BuildView`` (capped by the emitter)."""
        return [str(e) for e in ((self.view or {}).get("errors") or [])]

    def resource_view(self) -> dict[str, Any]:
        """The structured view for a live ``data`` dashboard resource: the
        ``nix-build`` renderer plus the latest ``BuildView`` as its data. Before
        the first line lands, a minimal placeholder so the pane renders at once."""
        data = self.view or {
            "command": f"nix {self.label}" if self.label else "nix",
            "builds": [],
            "activities": [],
            "counts": {"planned": 0, "running": 0, "stopped": 0, "succeeded": 0, "failed": 0},
            "errors": [],
            "finished": self.done,
            "exitCode": self.returncode,
        }
        return {"renderer": _NIX_BUILD_RENDERER, "data": data}

    def tree(self) -> str:
        """A compact text summary of the build (rows and status counts), for a
        model-facing repr away from the dashboard."""
        view = self.view or {}
        counts: dict[str, int] = view.get("counts") or {}
        head = " · ".join(
            f"{n} {label}"
            for label, key in (
                ("running", "running"),
                ("done", "succeeded"),
                ("failed", "failed"),
                ("planned", "planned"),
            )
            if (n := counts.get(key))
        )
        lines = [head or "(starting…)"]
        for build in view.get("builds") or []:
            mark = {"succeeded": "✓", "failed": "✗", "running": "▶"}.get(build.get("status"), "·")
            phase = f" [{build['phase']}]" if build.get("phase") else ""
            lines.append(f"  {mark} {build.get('name', '?')}{phase}")
        return "\n".join(lines)

    def _repr_html_(self) -> str:
        state = "error" if self.error else "done" if self.done else "building…"
        return (
            "<pre style='font:12px ui-monospace,monospace;margin:0'>"
            f"{_html.escape(state)}\n{_html.escape(self.tree())}</pre>"
        )

    def __repr__(self) -> str:
        counts: dict[str, int] = (self.view or {}).get("counts") or {}
        state = "error" if self.error else "done" if self.done else "building…"
        return f"<BuildRun {self.label or ''} [{state}] {counts.get('succeeded', 0)} built>"


def _register_live(run_state: BuildRun) -> Resource | None:
    """Register ``run_state`` as a live ``data`` dashboard resource, if running in
    a kernel. Decoupled on purpose: outside the kernel (a test, a plain
    interpreter) the runtime is absent and this is a silent no-op."""
    try:
        from ix_notebook_mcp.runtime import register_resource
    except Exception:
        return None
    return register_resource(
        render=run_state.resource_view,
        title=f"nix {run_state.label}" if run_state.label else "nix",
        kind="data",
        alive=lambda: not run_state.done,
    )


async def run(
    args: list[str],
    *,
    cwd: str | None = None,
    live: bool = True,
    label: str | None = None,
) -> BuildRun:
    """Run ``nix <args>`` under the ``nix-web-monitor`` emitter, returning a
    :class:`BuildRun`.

    Streams onto the shared event loop (never blocks it). ``args`` is everything
    after ``nix`` (e.g. ``["build", ".#mcp"]``). The emitter (the single owner of
    internal-json parsing) spawns nix, folds the event stream into a build tree,
    and streams a compact ``BuildView`` per state change; each line updates the
    returned :class:`BuildRun`. With ``live`` (the default) the in-flight build
    shows up as a self-closing dashboard pane drawn by the native ``nix-build``
    renderer. Run it as a background job for a long build and sample the returned
    handle between turns.

    ``cwd`` defaults to the kernel process's working directory; pass ``cwd=`` to
    resolve a flake ref (``.#foo``) against a specific worktree.
    """
    run_state = BuildRun(label=label or (args[1] if len(args) > 1 else args[0] if args else "nix"))
    try:
        proc = await asyncio.create_subprocess_exec(
            _nix_web_monitor_bin(),
            "--emit",
            "ndjson",
            "--",
            *args,
            cwd=cwd,
            stdout=asyncio.subprocess.PIPE,
            # The emitter routes nix's own stdout to its stderr; keep it out of our
            # NDJSON parse by draining it to the parent's stderr (not merged into
            # stdout, which carries the BuildView lines).
            stderr=None,
        )
    except OSError as exc:
        # The emitter binary is missing or not executable (e.g. a bare interpreter
        # with IX_NIX_WEB_MONITOR_BIN unset and nothing on PATH). Settle the run so
        # a caller sees the failure and, critically, so a live resource registered
        # below would self-close -- but register only AFTER a successful spawn, so
        # a failed spawn leaks no forever-alive pane.
        run_state.done = True
        run_state.returncode = None
        run_state._spawn_error = f"could not run {_nix_web_monitor_bin()}: {exc}"
        return run_state
    # Register the live pane only now that the process exists: a spawn failure
    # above returns early, so `alive` (keyed off `done`) can never pin a pane open.
    if live:
        _register_live(run_state)
    assert proc.stdout is not None
    # Read raw chunks and split on newlines ourselves: a build-log line folded
    # into a BuildView can exceed asyncio's default 64 KiB StreamReader limit,
    # which would make `readline` raise and abort mid-stream. `finally` reaps the
    # process and lets the live resource self-close (`alive` keys off `done`).
    buf = b""
    try:
        while True:
            chunk = await proc.stdout.read(65536)
            if not chunk:
                break
            buf += chunk
            *complete, buf = buf.split(b"\n")
            for line in complete:
                run_state.feed(line.decode(errors="replace"))
        if buf:
            run_state.feed(buf.decode(errors="replace"))
    finally:
        # On cancellation (or any early exit) the child must be signaled, not just
        # awaited: a bare `await proc.wait()` would let a cancelled build keep
        # running to completion while this coroutine parks in the finally. Kill it
        # if it has not already exited, then reap.
        if proc.returncode is None:
            with contextlib.suppress(ProcessLookupError):
                proc.terminate()
        run_state.returncode = await proc.wait()
        run_state.done = True
    return run_state


async def build(attr: str, *flags: str, cwd: str | None = None, live: bool = True) -> BuildRun:
    """Convenience for :func:`run` of ``nix build <attr> [flags]``."""
    return await run(["build", attr, *flags], cwd=cwd, live=live, label=attr)


# Kinds of flake output that are keyed by system (`<kind>.<system>.<name>`); the
# rest (nixosConfigurations, overlays, ...) are keyed by name directly.
_SYSTEMED = frozenset(
    {"packages", "legacyPackages", "apps", "checks", "devShells", "bundlers", "formatter"}
)


def _current_system() -> str:
    """The nix system double for this host (e.g. ``aarch64-darwin``)."""
    import platform
    import sys as _sys

    machine = platform.machine()
    arch = {"arm64": "aarch64", "amd64": "x86_64"}.get(machine, machine)
    os_name = "darwin" if _sys.platform == "darwin" else "linux"
    return f"{arch}-{os_name}"


def _flake_show_rows(data: dict[str, Any], system: str) -> list[dict[str, Any]]:
    """Flatten ``nix flake show --json`` into one row per buildable attribute.

    Pure (no subprocess), so it is testable on a captured payload. Systemed
    kinds are filtered to ``system``; an omitted (not-evaluated) system shows up
    as an empty dict and is skipped.
    """
    rows: list[dict[str, Any]] = []

    def emit(kind: str, attr: str, leaf: object) -> None:
        leaf = leaf if isinstance(leaf, dict) else {}
        rows.append(
            {
                "kind": kind,
                "attr": attr,
                "type": leaf.get("type"),
                "description": leaf.get("description"),
            }
        )

    for kind, sub in data.items():
        if not isinstance(sub, dict):
            continue
        if kind in _SYSTEMED:
            branch = sub.get(system)
            if not isinstance(branch, dict) or not branch:
                continue
            if branch.get("type"):  # a single leaf (e.g. formatter.<system>)
                emit(kind, kind, branch)
            else:
                for name, leaf in branch.items():
                    emit(kind, name, leaf)
        else:
            for name, leaf in sub.items():
                emit(kind, name, leaf)

    rows.sort(key=lambda r: (r["kind"], r["attr"]))
    return rows


async def attrs(flake: str = ".", *, system: str | None = None, cwd: str | None = None) -> pl.DataFrame:
    """Catalog a flake's buildable attributes as a ``polars.DataFrame``
    (``kind``, ``attr``, ``type``, ``description``).

    Answers "what can I build, and what is it?" without guessing attr paths.
    Composes with the polars API (``.filter`` by kind, search ``description``)
    and renders as the dashboard's styled table. Runs ``nix flake show --json``
    on the event loop and keeps only the current ``system`` (override with
    ``system=``). Note: flake-show lists declared outputs, not ``passthru``
    sub-attributes.
    """
    system = system or _current_system()
    proc = await asyncio.create_subprocess_exec(
        "nix",
        "flake",
        "show",
        "--json",
        "--no-warn-dirty",
        flake,
        cwd=cwd,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    out, err = await proc.communicate()
    if proc.returncode != 0:
        raise RuntimeError(f"nix flake show failed: {err.decode('utf-8', 'replace').strip()}")
    return pl.DataFrame(
        _flake_show_rows(_json.loads(out), system),
        schema={"kind": pl.Utf8, "attr": pl.Utf8, "type": pl.Utf8, "description": pl.Utf8},
    )


def _eval_args(
    installable: str, *, apply: str | None = None, system: str | None = None, raw: bool = False
) -> list[str]:
    """Build the ``nix eval`` argv (pure, so the quoting is testable).

    ``{system}`` in ``installable`` is substituted with ``system`` (or the host's
    system), so ``.#checks.{system}.lint`` resolves without hardcoding the double.
    ``apply`` rides as its own argv element, never spliced into a shell string.
    """
    target = installable.replace("{system}", system or _current_system())
    args = ["eval", target, "--raw" if raw else "--json", "--no-warn-dirty"]
    if apply is not None:
        args += ["--apply", apply]
    return args


async def eval(
    installable: str = ".",
    *,
    apply: str | None = None,
    system: str | None = None,
    cwd: str | None = None,
    raw: bool = False,
) -> Any:  # noqa: ANN401 -- decoded JSON is genuinely dynamic (or a raw str)
    """Evaluate a Nix installable and return the result as a native Python value.

    The friction this removes: ``nix eval .#checks.aarch64-linux --apply
    'builtins.attrNames'`` makes you hand-quote a Nix function inside a shell
    string inside a Python string, and hardcode the system double. Here ``apply``
    is a plain Python string passed as its own argv element (``create_subprocess_exec``,
    no shell, so no quoting), and the JSON result decodes to native Python ready
    for polars::

        names = await nix.eval(".#checks.{system}", apply="builtins.attrNames")
        pl.Series("check", names)                    # by lines / as a frame

        desc = await nix.eval(".#mcp", apply="p: p.meta.description")

    ``installable`` is a flake ref or attr path; ``{system}`` in it is replaced
    with ``system`` (default: the host's system double). ``apply`` is a Nix
    function applied before serialization. ``raw=True`` returns the string
    verbatim (``nix eval --raw``, e.g. a derivation's ``outPath``) instead of
    decoding JSON. ``cwd`` resolves a relative flake ref against a worktree. Runs
    on the shared event loop, so it never blocks other jobs; raise on a non-zero
    exit with nix's own stderr.
    """
    args = _eval_args(installable, apply=apply, system=system, raw=raw)
    proc = await asyncio.create_subprocess_exec(
        "nix",
        *args,
        cwd=cwd,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    out, err = await proc.communicate()
    if proc.returncode != 0:
        raise RuntimeError(f"nix eval failed: {err.decode('utf-8', 'replace').strip()}")
    text = out.decode("utf-8", "replace")
    return text if raw else _json.loads(text)
