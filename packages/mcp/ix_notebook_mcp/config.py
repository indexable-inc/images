"""Immutable server configuration, decided once by the CLI and read back by the
kernel manager, dashboard, and MCP transport.

The CLI builds a :class:`Config`, stashes it via :func:`set_config`, and runs the
event loop; the other modules read it with :func:`config`. A module-level handoff
keeps the wiring simple. The object is frozen so nothing mutates after launch.
"""

from __future__ import annotations

import datetime
import ipaddress
import json
import os
import socket
import stat
import time
from dataclasses import dataclass
from pathlib import Path

# Every tailscale IPv4 lives in the CGNAT range; anything else claiming to be
# one is malformed or hostile output. See `is_tailnet_ipv4`.
_TAILNET_V4 = ipaddress.ip_network("100.64.0.0/10")


def is_tailnet_ipv4(value: str) -> bool:
    """Whether ``value`` is an IPv4 literal in tailscale's CGNAT range
    (100.64.0.0/10).

    The defense-in-depth gate on every address taken from ``tailscale status``
    output (index#1789 review): a malformed or spoofed status must not be able
    to hand ``0.0.0.0`` or a LAN address to a bind or a peer probe. Real
    parsing (``ipaddress``), not string sniffing, so ``0.0.0.0``, IPv6, and
    junk all read as False.
    """
    try:
        addr = ipaddress.ip_address(value)
    except ValueError:
        return False
    return isinstance(addr, ipaddress.IPv4Address) and addr in _TAILNET_V4


# The well-known port every ix-mcp serves its tailnet `/mesh` discovery
# endpoint on (index#1787), adjacent to the fleet's fixed `/api/exec` port 8799
# (src/fleet/fleet/cluster.py EXEC_PORT). The one definition: the server bind
# (ix_notebook_mcp.mesh) and the bundled `mesh` module's peer probes both read
# it through :func:`mesh_port`, so the two sides cannot drift.
DEFAULT_MESH_PORT = 8798


def mesh_port() -> int:
    """The mesh port; ``IX_MCP_MESH_PORT`` overrides the well-known default."""
    return int(os.environ.get("IX_MCP_MESH_PORT") or DEFAULT_MESH_PORT)


def mesh_enabled() -> bool:
    """Whether this server should advertise itself on the tailnet mesh.

    Default ON: joining the mesh must need zero config (index#1787), so the env
    var is an opt-out only. ``IX_MCP_MESH=0`` (or false/no/off) disables it.
    """
    return os.environ.get("IX_MCP_MESH", "").strip().lower() not in ("0", "false", "no", "off")


def server_version() -> str:
    """The build's source revision. The nix wrapper sets ``IX_BUILD_REV`` (the
    shared build-stamp name every ix tool reads; see
    ``doc/build-version/overview.md``) to the flake rev (``<commit>`` /
    ``<commit>-dirty``); a bare run reads "dev". The MCP ``serverInfo.version``
    (tools.py), the ``/mesh`` payload, and the kernel's ``api()`` catalog all
    report this one value, so a client, a mesh peer, and an agent in a cell see
    the same commit."""
    return os.environ.get("IX_BUILD_REV") or "dev"


def build_epoch() -> int | None:
    """The build's commit time (unix epoch seconds, Nix's ``self.lastModified``)
    from ``IX_BUILD_EPOCH``. ``None`` when unset, malformed, or the ``0``
    non-git sentinel, so an unknown epoch never renders as 1970."""
    raw = os.environ.get("IX_BUILD_EPOCH")
    try:
        epoch = int(raw) if raw else 0
    except ValueError:
        return None
    return epoch or None


# Abbreviated-revision length in the stamp; mirrors build-version's SHORT_REV_LEN
# so the Python and Rust stamps read identically.
_SHORT_REV_LEN = 12


def _humanize_ago(seconds: int) -> str:
    """``just now`` / ``5 minutes ago`` / ``2 days ago`` / ``1 year ago``; the
    Python port of build-version's ``humanize_ago`` (same buckets, so ix tools
    and the kernel phrase age identically). Spans under a minute and negative
    spans (build clock ahead of ours) collapse to ``just now``."""
    if seconds < 60:
        return "just now"
    if seconds < 3600:
        value, unit = seconds // 60, "minute"
    elif seconds < 86400:
        value, unit = seconds // 3600, "hour"
    elif seconds < 7 * 86400:
        value, unit = seconds // 86400, "day"
    elif seconds < 30 * 86400:
        value, unit = seconds // (7 * 86400), "week"
    elif seconds < 365 * 86400:
        value, unit = seconds // (30 * 86400), "month"
    else:
        value, unit = seconds // (365 * 86400), "year"
    return f"{value} {unit}{'' if value == 1 else 's'} ago"


def build_stamp(now: float | None = None) -> str:
    """One line identifying this build, the shape build-version renders into
    every ix tool's ``--version``: ``7e42ccdb1882 (2026-06-07, 2 days ago)``.
    A reproducible build has no wall-clock build time, so the "when" is the
    commit time, and the age is computed here at call time (against ``now``,
    injectable for tests). Degrades to the bare short rev when the epoch is
    unknown, and to ``dev`` outside the packaged wrapper.

    This is the in-band staleness signal for agents (index#2110): a documented
    helper or kwarg missing from a kernel whose stamp is days old points at a
    stale deploy, not a phantom API."""
    rev = server_version()
    short = rev[:_SHORT_REV_LEN]
    epoch = build_epoch()
    if epoch is None:
        return short
    date = datetime.datetime.fromtimestamp(epoch, tz=datetime.UTC).strftime("%Y-%m-%d")
    now_epoch = int(now if now is not None else time.time())
    return f"{short} ({date}, {_humanize_ago(now_epoch - epoch)})"


@dataclass(frozen=True)
class Config:
    # Directory the kernel runs in and notebooks/files are resolved against.
    workdir: Path

    # The dashboard HTTP bind address. The dashboard is read-only (it renders the
    # execution store), but the store can contain anything the agent ran, so the
    # default bind is this node's Tailscale IPv4 when Tailscale is up (tailnet is
    # the trust boundary) and loopback otherwise. IX_MCP_HOST overrides.
    host: str = "127.0.0.1"
    # The aiohttp read-only data API (/api/jobs|resources|cells|snapshot|exec)
    # embedders poll. No longer serves a human UI -- that is the Loro hub below.
    dashboard_port: int = 0
    # The Loro dashboard hub (the `dashboard` aggregator the CLI spawns) the human
    # opens: it renders this server's panes -- and every other producer's -- live.
    hub_port: int = 0
    # True only when IX_MCP_AUTO_DASHBOARD made this server spawn its own per-server
    # hub at `hub_port`. The data API's `/` only redirects to `hub_port` in that
    # mode; in the default mode `hub_port` is a reserved-but-unbound port, so
    # probing it could 302 to whatever unrelated process later reused it.
    auto_dashboard: bool = False

    # Host advertised in the dashboard URL (distinct from the bind: a wildcard
    # bind is not a usable URL host, so the CLI resolves a reachable name).
    advertised_host: str = "127.0.0.1"

    # Path to the SQLite execution store the kernel writes and the dashboard reads.
    store_path: Path | None = None

    # Session mode (`serve --session FILE` / `notebook FILE`): the store IS the
    # session file -- kept across restarts instead of wiped, checkpointed by the
    # kernel runtime, restored on reopen. None for an ephemeral server.
    session_path: Path | None = None
    # True when the session file already existed at launch, so the server must
    # restore (load the checkpoint, replay the gap) before running new cells.
    session_resume: bool = False

    # This machine's tailscale IPv4, resolved once by the CLI, or None when
    # tailscale is absent or its backend is down. The `/mesh` endpoint binds
    # ONLY this address (index#1787): the tailnet is the trust boundary, and
    # with no tailnet there is nothing to mesh over, so mesh serving is skipped
    # rather than widened to a LAN or wildcard bind.
    mesh_host: str | None = None

    # "stdio" (the default; what an MCP client launches), "http", or "none"
    # (the standalone notebook engine: kernel + dashboard, no MCP transport).
    transport: str = "stdio"
    mcp_http_host: str = "127.0.0.1"
    mcp_http_port: int = 8000

    # In stdio mode the CLI dups the real stdin/stdout to these fds before any
    # library can write to fd 1, so the MCP protocol owns them exclusively.
    stdin_fd: int | None = None
    stdout_fd: int | None = None

    # Shared bearer token gating the dashboard's `/api/exec` write path (a peer's
    # `fleet.in_kernel` runs code in this node's live kernel). None disables the
    # endpoint entirely; set, it must match the request's `Authorization: Bearer`.
    # Sourced from IX_MCP_EXEC_TOKEN(_FILE) by the CLI; the fleet service hands
    # every node the same secret.
    exec_token: str | None = None

    # Static API key gating the MCP streamable-HTTP transport: every request to
    # it must carry this value in `X-Api-Key` (or `Authorization: Bearer`, for
    # clients whose path preserves that header -- some egress proxies strip
    # Authorization for allowlisted domains, which is why X-Api-Key is primary).
    # None leaves HTTP unauthenticated, which the CLI only allows on a
    # loopback/tailnet bind (see `cli._http_bind_error`). `GET /health` stays
    # open either way so a fronting proxy can probe liveness without the
    # secret. Sourced from IX_MCP_API_KEY(_FILE) by the CLI; stdio ignores it.
    api_key: str | None = None

    # Trust the bound network (the tailnet) as the `/api/exec` auth boundary, so
    # `fleet.in_kernel` works without a token -- the same model Ray's own data
    # plane relies on. The endpoint honors this only when `host` is non-loopback
    # (a tailnet/LAN bind, not 127.0.0.1). A set `exec_token` still wins (it is
    # then additionally required). With neither, the endpoint stays disabled.
    # Sourced from IX_MCP_EXEC_TRUST_NETWORK by the CLI; the fleet service sets it.
    # It ALSO gates `/api/input` (the interactive-resource write path): on a
    # non-loopback bind, input is accepted only when this is set (loopback always
    # accepts it). See `dashboard.input_submit`.
    exec_trust_network: bool = False

    # Seconds past a cell's own ``budget`` that the server waits for the kernel to
    # report idle before treating it as wedged by a synchronous call, interrupting
    # the kernel, and returning an actionable summary. See ``kernel.python_exec``.
    wedge_grace: float = 15.0

    # Per-cell static type checking (ty) before a cell executes: default on, so a
    # type error is caught and returned instead of blowing up at runtime. The
    # ``IX_MCP_TYPECHECK`` env var overrides this at the kernel (see
    # ``runtime._typecheck_enabled``); set this False to disable it server-wide.
    typecheck: bool = True

    # Hard ceiling on a single ``python_exec`` foreground ``budget``. The budget is
    # how long the ONE shared shell channel is held before the run backgrounds, so
    # an oversized budget (a 15-minute ``await jobs['x']``) wedges every other call
    # behind it for that whole time. Clamp it: a longer wait backgrounds and is
    # resumed by polling ``jobs['x']`` in a later cell. Raise it with a reason if a
    # workload genuinely needs a longer foreground hold.
    max_budget: float = 120.0

    def dashboard_url(self) -> str:
        """The read-only data API base (embedders poll /api/* here)."""
        return f"http://{self.advertised_host}:{self.dashboard_port}/"

    def hub_url(self) -> str:
        """The human-facing Loro dashboard the CLI spawns and advertises."""
        return f"http://{self.advertised_host}:{self.hub_port}/"

    def resolve(self, rel_path: str) -> Path:
        candidate = (self.workdir / rel_path).resolve()
        workdir = self.workdir.resolve()
        if workdir != candidate and workdir not in candidate.parents:
            raise ValueError(f"path {rel_path!r} escapes the workspace")
        return candidate


_CONFIG: Config | None = None


def set_config(value: Config) -> None:
    global _CONFIG
    _CONFIG = value


def config() -> Config:
    if _CONFIG is None:
        raise RuntimeError("config is unset; ix-mcp must be launched via its CLI")
    return _CONFIG


def runtime_dir() -> Path:
    """A private 0700 writable dir for the store and the dashboard-url handoff.

    Hardened against an untrusted shared base (/tmp): fail closed if a
    pre-existing one is a symlink, not ours, or group/other accessible (CWE-377).
    """
    base = os.environ.get("XDG_RUNTIME_DIR") or os.environ.get("TMPDIR") or "/tmp"  # noqa: S108 -- temp dir is hardened by the mkdir+stat checks below
    path = Path(base) / "ix-mcp"
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    info = path.lstat()
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
        raise RuntimeError(f"runtime dir {path} is not a real directory")
    if info.st_uid != os.getuid():
        raise RuntimeError(f"runtime dir {path} is not owned by the current user")
    if info.st_mode & 0o077:
        path.chmod(0o700)
    return path


def hub_state_path() -> Path:
    """Where the ``ix-mcp dashboard`` launcher records the one shared hub it
    started (``{pid, host, port, url}``), so a second launch reuses it instead of
    spawning another. Machine-wide, distinct from any single ``serve``'s random
    ``hub_port``: there is exactly one shared hub, not one per session."""
    return runtime_dir() / "hub.json"


def port_open(port: int, host: str = "127.0.0.1", timeout: float = 0.25) -> bool:
    """Whether something is accepting TCP connections on ``host:port`` right now.
    Distinct from ``cli._bindable`` (which asks whether the port is *free*): this
    asks whether a server is *live* there, so a stale hub-state file pointing at a
    dead port is detected and ignored."""
    try:
        with socket.create_connection((host, port), timeout=timeout):
            return True
    except OSError:
        return False


def live_hub() -> dict | None:
    """The shared dashboard hub's advertised state if one is actually running,
    else ``None``. Reads :func:`hub_state_path` and confirms the port is live, so
    a leftover file from a crashed hub is treated as no hub. Cheap when no file
    exists (the common case): no socket probe happens unless a record is present.

    Probes the recorded bind ``host`` (not a hardcoded loopback) so a hub bound to
    this machine's tailnet IP is correctly seen as live; a wildcard bind is probed
    via loopback."""
    try:
        data = json.loads(hub_state_path().read_text())
    except (OSError, ValueError):
        return None
    # The recorded process must still be alive: a TCP listener on the port alone
    # is not enough, since after the hub dies an unrelated service could bind the
    # same (often default 8080) port and we would redirect users to it. A dead pid
    # means the file is stale, regardless of who now holds the port.
    pid = data.get("pid")
    if isinstance(pid, int) and pid > 0:
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return None
        except PermissionError:
            pass  # exists but owned by another user -- still alive
    port = data.get("port")
    host = data.get("host") or "127.0.0.1"
    probe = "127.0.0.1" if host in ("0.0.0.0", "::", "") else host  # noqa: S104 -- mapping a wildcard record to a probeable loopback, not a bind
    if not isinstance(port, int) or not port_open(port, probe):
        return None
    return data
