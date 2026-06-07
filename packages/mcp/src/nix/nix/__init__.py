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
  subprocess, so it is trivially testable and works on a captured log.
* :func:`run` / :func:`build` -- async: spawn ``nix`` with ``--log-format
  internal-json`` on the shared event loop, feed a :class:`NixLog` line by line,
  and (in a kernel session) register it as a live dashboard **Resource** so the
  human watches the DAG grow and self-close when the build finishes. The model
  gets the finished :class:`NixLog` back -- ``log.activities`` / ``log.events``
  are polars frames, ``log.tree()`` is the rendered DAG.

Run it as a background job (``await nix.build(".#foo")`` runs past the budget and
backgrounds); next turn ``await jobs['..']`` yields the finished :class:`NixLog`
(``log.activities`` / ``log.events`` / ``log.tree()``), while the dashboard
Resource shows the DAG growing live as it builds.
"""

from __future__ import annotations

import asyncio
import html as _html
import json as _json
import re
import time
from collections.abc import Iterable

import polars as pl

__all__ = ["NixLog", "parse", "run", "build", "attrs", "ACTIVITY_TYPES", "RESULT_TYPES"]

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
    try:
        return int(v)  # type: ignore[arg-type]
    except (TypeError, ValueError):
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
        self._acts: dict[int, dict] = {}
        self._order: list[int] = []
        self.error: str | None = None
        self.done = False
        self.returncode: int | None = None
        self.started = time.time()

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
            fields = o.get("fields") or []
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
        by_parent: dict[int, list[dict]] = {}
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


def _register_live(log: NixLog):
    """Register ``log`` as a live dashboard Resource, if running in a kernel.

    Decoupled on purpose: outside the kernel (a test, a plain interpreter) the
    runtime is absent and this is a silent no-op, so the parser stays pure.
    """
    try:
        from ix_notebook_mcp.runtime import register_resource
    except Exception:
        return None
    return register_resource(
        render=log.resource_html,
        title=f"nix {log.label}" if log.label else "nix",
        kind="html",
        alive=lambda: not log.done,
    )


async def run(
    args: list[str],
    *,
    cwd: str | None = None,
    live: bool = True,
    label: str | None = None,
) -> NixLog:
    """Run ``nix <args>`` with ``internal-json`` logging, returning a NixLog.

    Streams onto the shared event loop (never blocks it). ``args`` is everything
    after ``nix`` (e.g. ``["build", ".#mcp"]``); ``--log-format internal-json`` is
    appended. With ``live`` (the default) the in-flight log shows up as a
    self-closing dashboard Resource. Run it as a background job for a long build
    and sample the returned NixLog between turns.

    ``cwd`` defaults to the kernel process's working directory; pass ``cwd=`` to
    resolve a flake ref (``.#foo``) against a specific worktree.
    """
    log = NixLog(label=label or (args[1] if len(args) > 1 else args[0] if args else "nix"))
    if live:
        _register_live(log)
    proc = await asyncio.create_subprocess_exec(
        "nix",
        *args,
        "--log-format",
        "internal-json",
        cwd=cwd,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.STDOUT,
    )
    assert proc.stdout is not None
    # Read raw chunks and split on newlines ourselves: a single `buildLogLine`
    # (a compiler/test/minified line) can exceed asyncio's default 64 KiB
    # StreamReader limit, which would make `async for`/`readline` raise and
    # abort mid-stream. `finally` guarantees the process is reaped and the live
    # Resource self-closes (`alive` keys off `log.done`) on every exit path.
    buf = b""
    try:
        while True:
            chunk = await proc.stdout.read(65536)
            if not chunk:
                break
            buf += chunk
            *complete, buf = buf.split(b"\n")
            for line in complete:
                log.feed(line.decode(errors="replace"))
        if buf:
            log.feed(buf.decode(errors="replace"))
    finally:
        log.returncode = await proc.wait()
        log.done = True
        if log.returncode and not log.error:
            log.error = f"nix exited with code {log.returncode}"
    return log


async def build(attr: str, *flags: str, cwd: str | None = None, live: bool = True) -> NixLog:
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


def _flake_show_rows(data: dict, system: str) -> list[dict]:
    """Flatten ``nix flake show --json`` into one row per buildable attribute.

    Pure (no subprocess), so it is testable on a captured payload. Systemed
    kinds are filtered to ``system``; an omitted (not-evaluated) system shows up
    as an empty dict and is skipped.
    """
    rows: list[dict] = []

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
        _flake_show_rows(json.loads(out), system),
        schema={"kind": pl.Utf8, "attr": pl.Utf8, "type": pl.Utf8, "description": pl.Utf8},
    )
