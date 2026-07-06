"""The ``ix-mcp`` command line.

  ix-mcp serve                 run the MCP server over stdio (what a client launches)
  ix-mcp serve --http A        run it over streamable HTTP at A (host:port)
  ix-mcp serve --session F     same, but F is a persistent session file (see below)
  ix-mcp notebook [F]          run the notebook engine alone (kernel + dashboard, no MCP)
  ix-mcp eval EXPR             evaluate one expression on a throwaway kernel
  ix-mcp exec SRC              run statements on a throwaway kernel

`serve` starts ONE shared IPython kernel, a read-only data API over the
execution store, and the MCP transport, all on one event loop. It publishes its
panes into the shared discovery dir but does NOT spawn a `dashboard` hub: that
aggregator is a single machine-wide process, started once on demand by
`ix-mcp dashboard` (which reuses a running hub or spawns one and opens it), and
renders every server behind one URL. Set `IX_MCP_AUTO_DASHBOARD` truthy to
restore the old per-server auto-spawn.
`notebook` is the engine without the MCP surface: the same kernel and store,
driven only by what is already in the session file and the humans watching it.

A session file (``--session work.ixnb`` or ``notebook work.ixnb``) makes the
store persistent instead of per-run: every cell, its outputs, and a serialized
checkpoint of the kernel namespace are recorded in that one SQLite file.
Reopening an existing file restores the namespace from the checkpoint
instantly, replays only the cells newer than it, and marks cells that died
mid-run as interrupted -- a Jupyter notebook whose state comes back.
"""

from __future__ import annotations

import argparse
import asyncio
import fcntl
import ipaddress
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import time
import webbrowser
from collections.abc import Callable
from pathlib import Path

from .config import (
    Config,
    hub_state_path,
    is_tailnet_ipv4,
    live_hub,
    port_open,
    runtime_dir,
    set_config,
)

_ANSI = re.compile(r"\x1b\[[0-9;]*m")
_WILDCARD_HOSTS = {"0.0.0.0", "::"}  # noqa: S104 -- deliberate set of wildcard host strings for comparison


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="ix-mcp", description="Single-tool Python execution MCP server")
    sub = parser.add_subparsers(dest="command")

    serve = sub.add_parser("serve", help="Run the MCP server")
    serve.add_argument("--workdir", help="Directory the kernel runs in (default: cwd)")
    serve.add_argument(
        "--http",
        nargs="?",
        const="127.0.0.1:8000",
        metavar="ADDR",
        help="Serve over streamable HTTP at host:port instead of stdio",
    )
    serve.add_argument(
        "--session",
        metavar="FILE",
        help="Persistent session file: record every cell, its outputs, and a namespace "
        "checkpoint there; reopening an existing file restores the state",
    )
    notebook = sub.add_parser(
        "notebook", help="Run the notebook engine alone (kernel + dashboard, no MCP transport)"
    )
    notebook.add_argument(
        "session", nargs="?", metavar="FILE", help="Session file to create or reopen"
    )
    notebook.add_argument("--workdir", help="Directory the kernel runs in (default: cwd)")
    dash = sub.add_parser(
        "dashboard",
        help="Open the shared dashboard UI, starting it once if it is not already running",
    )
    dash.add_argument(
        "--no-open",
        action="store_true",
        help="Print the URL but do not open a browser",
    )
    sub.add_parser(
        "requirements",
        help="Report each external credential the bundled tooling needs: present "
        "(and from where) or missing (and the remedy); exits non-zero when "
        "anything is missing, so setup scripts can gate on it",
    )
    ev = sub.add_parser("eval", help="Evaluate one expression on a throwaway kernel")
    ev.add_argument("code")
    ex = sub.add_parser("exec", help="Run statements on a throwaway kernel")
    ex.add_argument("code")

    args = parser.parse_args(argv)
    command = args.command or "serve"
    if command in ("serve", "notebook"):
        return _serve(args, engine_only=command == "notebook")
    if command == "dashboard":
        return _dashboard(open_browser=not args.no_open)
    if command == "requirements":
        from . import requirements

        return 0 if requirements.report(print) else 1
    if command in ("eval", "exec"):
        return _one_shot(args.code)
    parser.error(f"unknown command {command!r}")
    return 2


def _prepare_ipython_startup(tag: int) -> Path:
    """Materialize a private IPYTHONDIR whose startup folder holds the shipped
    ``ipython/`` scripts, so the in-kernel runtime + polars tweak load in the
    kernel. Isolated under the 0700 runtime dir, per-tag so concurrent servers
    do not share IPython state."""
    assets = Path(__file__).resolve().parent / "ipython"
    base = runtime_dir() / f"ipython-{tag}"
    startup = base / "profile_default" / "startup"
    # Clear first: if a previous server reused this tag (the OS can reassign the
    # same port), a stale startup script from an older build (e.g. the removed
    # itables one) would otherwise still run in the kernel.
    if startup.exists():
        shutil.rmtree(startup)
    startup.mkdir(parents=True, exist_ok=True)
    for script in sorted(assets.glob("*.py")):
        shutil.copyfile(script, startup / script.name)
    return base


def _free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def _bindable(host: str, port: int) -> bool:
    """Whether ``host:port`` can actually be bound right now. A configured host
    can be 'assigned' yet unbindable -- e.g. a Tailscale IP whose interface is
    down because the backend is stopped -- so the CLI probes before committing
    the dashboard (and the kernel's inherited URL) to it. Mirrors what
    ``loop.create_server`` does: resolve, then try each address family."""
    try:
        infos = socket.getaddrinfo(host, port, type=socket.SOCK_STREAM)
    except OSError:
        return False
    for family, socktype, proto, _canon, sockaddr in infos:
        try:
            with socket.socket(family, socktype, proto) as sock:
                sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
                sock.bind(sockaddr)
            return True
        except OSError:
            continue
    return False


def _dashboard_port() -> int:
    """The port the read-only data API / dashboard binds. An embedder (the room
    server runs ``ix-mcp`` as its agent's tool and reads results back over HTTP)
    pins it with ``IX_MCP_DASHBOARD_PORT`` so it knows where to reach this
    instance; left unset, a free port is chosen so a bare run never collides."""
    pinned = os.environ.get("IX_MCP_DASHBOARD_PORT")
    if pinned:
        return int(pinned)
    return _free_port()


def _store_path(dashboard_port: int) -> Path:
    """The SQLite execution store. An embedder (the pi-harness room event
    mapper polls the store for cell/resource updates) pins it with
    ``IX_MCP_STORE`` so both sides agree on one file; the path is used
    verbatim and the pinning caller owns its parent directory. Left unset,
    the store lives in the private runtime dir, keyed by the data-API port
    so concurrent servers never collide."""
    pinned = os.environ.get("IX_MCP_STORE")
    if pinned:
        return Path(pinned)
    return runtime_dir() / f"store-{dashboard_port}.db"


def _stat_exists(path: str) -> bool:
    """``Path.exists`` that treats an unstatable path as absent: a sandboxed
    or hardened host can deny even ``stat`` on ``/usr`` (PermissionError, not
    False), and tailscale discovery must degrade to "none", never crash."""
    try:
        return Path(path).exists()
    except OSError:
        return False


def _tailscale_status() -> dict | None:
    tailscale = shutil.which("tailscale") or next(
        (p for p in ("/usr/local/bin/tailscale", "/usr/bin/tailscale") if _stat_exists(p)), None
    )
    if not tailscale:
        return None
    try:
        out = subprocess.run(
            [tailscale, "status", "--json"], capture_output=True, text=True, timeout=2, check=True
        ).stdout
        return json.loads(out)
    except Exception:
        return None


def _tailscale_dns_name() -> str | None:
    status = _tailscale_status()
    if not status:
        return None
    name = status.get("Self", {}).get("DNSName", "").rstrip(".")
    return name or None


def _tailscale_ip() -> str | None:
    status = _tailscale_status()
    if not status:
        return None
    # A stopped backend still reports its assigned IPs, but they are not bound to
    # any interface, so binding the dashboard to one fails. Only treat the IP as
    # usable when Tailscale is actually up.
    if status.get("BackendState") != "Running":
        return None
    for ip in status.get("Self", {}).get("TailscaleIPs", []) or []:
        # CGNAT-only (100.64.0.0/10): a malformed or spoofed status must not be
        # able to steer a bind to 0.0.0.0 or a LAN address (index#1789 review).
        # Every real tailscale IPv4 is CGNAT, so nothing legitimate is lost.
        if isinstance(ip, str) and is_tailnet_ipv4(ip):
            return ip
    return None


def _advertised_host(bind_host: str) -> str:
    public = os.environ.get("IX_MCP_PUBLIC_HOST")
    if public:
        return public
    if bind_host not in _WILDCARD_HOSTS:
        return bind_host
    dns = _tailscale_dns_name()
    if dns:
        return dns
    fqdn = socket.getfqdn()
    if "." in fqdn and fqdn != "localhost":
        return fqdn
    return "127.0.0.1"


# The path where 1Password puts its SSH agent socket on macOS.
_1PASSWORD_AGENT_SOCK = "Library/Group Containers/2BUA8C4S2C.com.1password/t/agent.sock"


def _resolve_ssh_auth_sock(
    current: str | None,
    home: Path,
    platform: str,
    exists: Callable[[str], bool] = lambda p: Path(p).exists(),
) -> str | None:
    """Return the 1Password agent socket path to use instead of *current*, or
    ``None`` if no substitution should be made.

    Substitution happens only when ALL of the following are true:
    - the platform is Darwin (macOS),
    - the 1Password agent socket exists under *home*, and
    - *current* is either unset or points at the empty Apple launchd SSH agent
      (its path always contains ``com.apple.launchd.``).

    A deliberately set non-Apple agent (e.g. a custom ``SSH_AUTH_SOCK`` the
    user exported) is never overridden.
    """
    if platform != "darwin":
        return None
    op_sock = str(home / _1PASSWORD_AGENT_SOCK)
    if not exists(op_sock):
        return None
    # Don't clobber a custom, non-Apple agent.
    if current and "com.apple.launchd." not in current:
        return None
    return op_sock


def _exec_token() -> str | None:
    """The shared secret gating `/api/exec` (a peer's `fleet.in_kernel`).

    From ``IX_MCP_EXEC_TOKEN`` directly, or a file named by
    ``IX_MCP_EXEC_TOKEN_FILE`` (the fleet service keeps the secret in a file and
    points every node at it). Unset, the exec endpoint stays disabled.
    """
    token = os.environ.get("IX_MCP_EXEC_TOKEN")
    if token:
        return token.strip()
    path = os.environ.get("IX_MCP_EXEC_TOKEN_FILE")
    if path and Path(path).exists():
        return Path(path).read_text().strip()
    return None


def _api_key() -> str | None:
    """The static API key gating the MCP streamable-HTTP transport.

    From ``IX_MCP_API_KEY`` directly, or a file named by ``IX_MCP_API_KEY_FILE``
    (a deployment keeps the secret in a root-only file or env unit and points
    the server at it; the key is never baked into a config in the repo). Every
    HTTP request must then carry it in ``X-Api-Key`` (or ``Authorization:
    Bearer`` where that header survives the client's path). Unset, the HTTP
    transport stays unauthenticated, which :func:`_http_bind_error` only allows
    on a loopback/tailnet bind. stdio mode never reads this.
    """
    key = os.environ.get("IX_MCP_API_KEY")
    if key:
        return key.strip()
    path = os.environ.get("IX_MCP_API_KEY_FILE")
    if path and Path(path).exists():
        return Path(path).read_text().strip()
    return None


def _http_bind_error(host: str, api_key: str | None) -> str | None:
    """Why serving MCP over HTTP at ``host`` must be refused, or None if allowed.

    With an API key configured every bind is fine: each request authenticates
    itself, so the reachability of the port is not the trust boundary. Without
    one, only binds whose reachability already IS a trust boundary are allowed
    -- loopback (a local client or a fronting reverse proxy) or this node's
    tailnet interface (the same model the dashboard and `/api/exec` use). A
    wildcard, LAN, or public bind with no key would hand the kernel to anyone
    who can reach the port, so it is refused outright rather than served open.
    """
    if api_key is not None:
        return None
    if host not in _WILDCARD_HOSTS:
        if host == "localhost" or is_tailnet_ipv4(host):
            return None
        try:
            if ipaddress.ip_address(host).is_loopback:
                return None
        except ValueError:
            pass  # a hostname we cannot classify is treated as public
    return (
        f"refusing --http on {host!r} without an API key: the MCP endpoint would "
        "expose the kernel to anyone who can reach the port. Set IX_MCP_API_KEY "
        "(or IX_MCP_API_KEY_FILE), or bind loopback/tailnet instead"
    )


def _exec_trust_network() -> bool:
    """Whether to trust the bound network (the tailnet) as the `/api/exec` auth
    boundary, so a peer's `fleet.in_kernel` works without a shared token -- the
    same trust model Ray's own data plane relies on. Off unless
    ``IX_MCP_EXEC_TRUST_NETWORK`` is set truthy; the dashboard additionally
    requires a non-loopback bind before honoring it.
    """
    return os.environ.get("IX_MCP_EXEC_TRUST_NETWORK", "").strip().lower() in (
        "1",
        "true",
        "yes",
        "on",
    )


def _auto_dashboard() -> bool:
    """Whether ``serve`` should spawn its own ``dashboard`` hub.

    Off by default. The hub is a single, machine-wide, long-lived aggregator:
    exactly one process binds the port, and every ``serve`` already publishes its
    panes into the shared discovery dir (see ``pane_bridge``), so one hub renders
    them all behind one stable URL. Spawning a hub per ``serve`` both duplicates
    that singleton and leaks it -- a ``serve`` killed abnormally (SIGKILL/crash)
    skips the cleanup ``finally``, so its hub reparents to init and survives
    forever; a churning fleet of short-lived servers piles up thousands of
    orphaned ``dashboard`` processes. Run the UI yourself once instead
    (``nix run .#dashboard``). Set ``IX_MCP_AUTO_DASHBOARD`` truthy to restore the
    old auto-spawn.
    """
    return os.environ.get("IX_MCP_AUTO_DASHBOARD", "").strip().lower() in (
        "1",
        "true",
        "yes",
        "on",
    )


def _serve(args: argparse.Namespace, *, engine_only: bool = False) -> int:
    wd = getattr(args, "workdir", None)
    workdir = Path(wd).resolve() if wd else Path.cwd()
    workdir.mkdir(parents=True, exist_ok=True)

    http = getattr(args, "http", None)
    stdin_fd = stdout_fd = None
    mcp_http_host, mcp_http_port = "127.0.0.1", 8000
    if engine_only:
        # `notebook`: the engine alone, no MCP transport at all.
        transport = "none"
    elif http is None:
        # Hand the MCP protocol the real stdin/stdout, then point fd 0/1 at
        # /dev/null and stderr so nothing else can corrupt the JSON-RPC stream.
        stdin_fd = os.dup(0)
        stdout_fd = os.dup(1)
        os.dup2(2, 1)
        devnull = os.open(os.devnull, os.O_RDONLY)
        os.dup2(devnull, 0)
        os.close(devnull)
        transport = "stdio"
    else:
        transport = "http"
        host, _, port = http.partition(":")
        mcp_http_host, mcp_http_port = host or "127.0.0.1", int(port) if port else 8000

    api_key = _api_key() if transport == "http" else None
    if transport == "http":
        bind_error = _http_bind_error(mcp_http_host, api_key)
        if bind_error:
            print(f"[ix-mcp] {bind_error}", file=sys.stderr)
            return 2

    dashboard_port = _dashboard_port()

    # Resolve the dashboard bind host once, here, before the kernel spawns (it
    # inherits IX_MCP_DASHBOARD_URL) and before the Config is built, so every
    # derived value stays consistent. A tailnet IP can be 'assigned' yet
    # unbindable (Tailscale stopped -> interface down); probing and falling back
    # to loopback keeps the read-only dashboard, hence the whole MCP, from
    # crashing on startup. _tailscale_ip() already returns None unless the
    # backend is running, so this only catches the rarer races.
    bind_host = os.environ.get("IX_MCP_HOST") or _tailscale_ip() or "127.0.0.1"
    # Probe everything except the fallback target itself. "127.0.0.1" is the host
    # we fall back *to* and is effectively always bindable; every other spelling
    # (a tailnet IP, but also "::1"/"localhost", which can be down when IPv6 is
    # disabled) must be probed so we degrade to working loopback instead of it.
    if bind_host != "127.0.0.1" and not _bindable(bind_host, dashboard_port):
        print(
            f"[ix-mcp] dashboard host {bind_host}:{dashboard_port} is not bindable; "
            "falling back to 127.0.0.1",
            file=sys.stderr,
            flush=True,
        )
        bind_host = "127.0.0.1"
    advertised_host = _advertised_host(bind_host)

    session = getattr(args, "session", None)
    session_path: Path | None = None
    session_resume = False
    if session:
        if os.environ.get("IX_MCP_STORE"):
            print(
                "--session and IX_MCP_STORE both pin the store file; set only one",
                file=sys.stderr,
            )
            return 2
        # The session file IS the store: one SQLite file carrying the cells,
        # their outputs, and the namespace checkpoint. Kept across restarts --
        # persistence is the point -- where an ephemeral store is wiped below.
        session_path = Path(session).expanduser().resolve()
        session_path.parent.mkdir(parents=True, exist_ok=True)
        store_path = session_path
        session_resume = session_path.exists()
        os.environ["IX_MCP_SESSION"] = "1"
        if session_resume:
            from . import store as store_mod

            conn = store_mod.connect(store_path)
            try:
                stale = store_mod.mark_interrupted(conn, ended_at=time.time())
            finally:
                conn.close()
            if stale:
                print(
                    f"[ix-mcp] session {store_path.name}: {stale} cell(s) from the "
                    "previous run marked interrupted",
                    file=sys.stderr,
                    flush=True,
                )
    else:
        # Ephemeral mode: make sure a stale flag inherited from a parent's env
        # cannot switch the kernel runtime into checkpointing.
        os.environ.pop("IX_MCP_SESSION", None)
        store_path = _store_path(dashboard_port)
        # Fresh execution log per ephemeral server, pinned or minted: a leftover
        # database (and WAL sidecars) from a prior run would otherwise show
        # stale runs in the dashboard and the room feed.
        for suffix in ("", "-wal", "-shm"):
            (store_path.parent / (store_path.name + suffix)).unlink(missing_ok=True)

    # The Loro hub (human UI) binds its own port; the aiohttp data API keeps
    # dashboard_port. A pinned IX_MCP_HUB_PORT lets an embedder reach a known hub.
    hub_port = int(os.environ.get("IX_MCP_HUB_PORT") or _free_port())

    cfg = Config(
        workdir=workdir,
        host=bind_host,
        advertised_host=advertised_host,
        dashboard_port=dashboard_port,
        hub_port=hub_port,
        # The `/mesh` endpoint binds ONLY the tailscale IP (index#1787): unlike
        # `bind_host` above there is no loopback fallback, because a
        # loopback-only mesh card is unreachable by every peer and a wider bind
        # would leave the trust boundary. None -> mesh.start skips serving.
        mesh_host=_tailscale_ip(),
        auto_dashboard=_auto_dashboard(),
        store_path=store_path,
        session_path=session_path,
        session_resume=session_resume,
        transport=transport,
        mcp_http_host=mcp_http_host,
        mcp_http_port=mcp_http_port,
        stdin_fd=stdin_fd,
        stdout_fd=stdout_fd,
        api_key=api_key,
        exec_token=_exec_token(),
        exec_trust_network=_exec_trust_network(),
    )
    set_config(cfg)

    # The kernel inherits this process's env, so set the store path (the runtime
    # writes there) and the private IPYTHONDIR (so the runtime startup runs)
    # before the kernel starts.
    os.environ["IX_MCP_STORE"] = str(store_path)
    # Surface the dashboard URL to the kernel so `DASHBOARD_URL` is one lookup
    # away (the agent should not have to spelunk the runtime dir to find it). The
    # human-facing dashboard is the Loro hub when we auto-spawn one; otherwise
    # there is no per-server hub, so point at this server's own data API.
    os.environ["IX_MCP_DASHBOARD_URL"] = cfg.hub_url() if _auto_dashboard() else cfg.dashboard_url()
    # The data API base, ALWAYS this server's own read/write API (never the hub):
    # the runtime bakes it into an interactive resource's `ixSubmit` so the
    # browser posts user input to `/api/input` here. Distinct from
    # IX_MCP_DASHBOARD_URL above, which may point at the human-facing hub.
    os.environ["IX_MCP_DATA_API_URL"] = cfg.dashboard_url()
    os.environ["IPYTHONDIR"] = str(_prepare_ipython_startup(dashboard_port))

    # On macOS the process env inherits the empty Apple launchd SSH agent
    # socket, not the 1Password agent that op-ssh-sign (the configured
    # gpg.ssh.program) needs for signed git commits.  Redirect SSH_AUTH_SOCK
    # to the 1Password socket when it exists so every sh(...) subprocess -- and
    # git commit signing -- work without manual overrides.
    _op_sock = _resolve_ssh_auth_sock(
        os.environ.get("SSH_AUTH_SOCK"),
        Path.home(),
        sys.platform,
    )
    if _op_sock is not None:
        os.environ["SSH_AUTH_SOCK"] = _op_sock
        print(f"[ix-mcp] SSH_AUTH_SOCK -> 1Password agent ({_op_sock})", file=sys.stderr, flush=True)

    # Yell about missing credentials once, up front, on the channel MCP clients
    # surface in their logs. Each module still fails clearly per call; this is
    # the advance warning so the gap is visible before the first call hits it.
    from . import requirements

    requirements.report(lambda line: print(f"[ix-mcp] {line}", file=sys.stderr, flush=True))

    asyncio.run(_run(cfg))
    return 0


def _spawn_hub(cfg: Config) -> subprocess.Popen | None:
    """Spawn the Loro dashboard hub (the `dashboard` aggregator) the human opens.

    It watches the shared discovery directory the pane bridge publishes into, so
    it renders this server's panes alongside every other producer (a TUI's
    terminals, a VM's screen). Best-effort: if the binary is absent (a bare run
    outside nix, which bundles it on PATH), log and skip -- the read-only data
    API still serves embedders, there is just no UI."""
    hub_bin = _hub_bin()
    if not hub_bin:
        print(
            "[ix-mcp] dashboard hub binary not found; UI disabled "
            "(build via nix, which bundles `dashboard`, or run it yourself)",
            file=sys.stderr,
            flush=True,
        )
        return None
    try:
        # `--record-ms 0`: do NOT persist the board to disk. The hub aggregates
        # every producer's panes (this kernel's namespace values, captured
        # outputs, terminals) -- recording them to a replay file is surprising,
        # potentially-sensitive persistence for an ephemeral MCP session. Live
        # replay within the open browser session still works.
        return subprocess.Popen(
            [hub_bin, "--host", cfg.host, "--port", str(cfg.hub_port), "--record-ms", "0"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    except OSError as error:
        print(f"[ix-mcp] failed to start dashboard hub: {error}", file=sys.stderr, flush=True)
        return None


async def _run(cfg: Config) -> None:
    from . import dashboard, mesh, pane_bridge, tools, transport
    from .kernel import Kernel, set_kernel

    kernel = Kernel(cfg)
    await kernel.start()
    set_kernel(kernel)

    restore_task: asyncio.Task | None = None
    if cfg.session_resume:
        # Reopen the session in the kernel. Runs as a task so the transport can
        # come up immediately, but only after the restore HOLDS the shell
        # channel (the `locked` event): every tool call submitted later queues
        # behind it on the kernel lock, so the first cell of the new run always
        # sees the restored state.
        locked = asyncio.Event()

        async def _restore() -> None:
            try:
                summary = await kernel.restore_session(on_locked=locked.set)
                if summary:
                    print(f"[ix-mcp] {summary}", file=sys.stderr, flush=True)
            except Exception as exc:
                print(f"[ix-mcp] session restore failed: {exc!r}", file=sys.stderr, flush=True)
            finally:
                locked.set()  # a restore that died before locking must not hang serving

        restore_task = asyncio.ensure_future(_restore())
        await locked.wait()

    runner = await dashboard.start(cfg)
    # Always publish this server's runs/resources/namespace as panes into the
    # shared discovery dir; a single standalone `dashboard` (run separately,
    # `nix run .#dashboard`) renders every producer behind one stable URL.
    bridge_task = asyncio.ensure_future(pane_bridge.run(cfg.store_path))
    # Only auto-spawn a per-server hub when explicitly opted in (see
    # `_auto_dashboard`): the default leaks an orphaned `dashboard` per abnormal
    # exit and duplicates the machine-wide singleton. Best-effort either way -- a
    # missing hub binary or an unbindable producer socket just means no UI here.
    hub = _spawn_hub(cfg) if _auto_dashboard() else None
    # Advertise the hub UI only if we actually started one; otherwise point at the
    # live data API rather than a dead hub port.
    url = cfg.hub_url() if hub is not None else cfg.dashboard_url()
    (runtime_dir() / "dashboard-url").write_text(url)
    # Bake the live URL into the MCP instructions before serving, so the client
    # gets it in the `initialize` response -- no tool call to discover it.
    tools.set_dashboard_url(url)
    # Advertise this server on the tailnet mesh (`GET /mesh`, index#1787):
    # default-on and best-effort -- no tailscale, an occupied port, or
    # IX_MCP_MESH=0 log one line and skip, never blocking the MCP itself.
    # Started only now, AFTER the hub-spawn decision resolved `url`, so the
    # card advertises the URL a human can actually open (a failed auto hub
    # falls back to the data API here, not to a dead pre-spawn hub URL --
    # index#1789 review).
    mesh_runner = await mesh.start(cfg, tools.session_names, url)
    if hub is not None:
        print(f"[ix-mcp] dashboard (all running things + output): {url}", file=sys.stderr, flush=True)
    else:
        print(
            f"[ix-mcp] data API: {url}  (open the UI: `ix-mcp dashboard`)",
            file=sys.stderr,
            flush=True,
        )
    if cfg.session_path is not None:
        print(f"[ix-mcp] session file: {cfg.session_path}", file=sys.stderr, flush=True)

    try:
        if cfg.transport == "none":
            # The standalone notebook engine: stay up until the process is
            # told to stop (Ctrl-C / SIGTERM).
            await asyncio.Event().wait()
        else:
            await transport.serve()
    finally:
        bridge_task.cancel()
        if hub is not None:
            hub.terminate()
            try:
                hub.wait(timeout=5)
            except subprocess.TimeoutExpired:
                hub.kill()
        if restore_task is not None and not restore_task.done():
            restore_task.cancel()
        if cfg.session_path is not None:
            # Final checkpoint so the last cells' state reopens instantly even
            # when the debounced checkpoint had not fired yet.
            await kernel.snapshot_session()
        if mesh_runner is not None:
            await mesh_runner.cleanup()
        await runner.cleanup()
        # Flush the final redundant-read stats while the kernel is still alive:
        # shutdown() kills it with SIGKILL, past which no in-kernel code runs.
        await kernel.emit_read_stats_final()
        await kernel.shutdown()


def _hub_bin() -> str | None:
    """The `dashboard` aggregator binary: the nix wrapper bakes IX_DASHBOARD_BIN,
    a bare run falls back to PATH."""
    return os.environ.get("IX_DASHBOARD_BIN") or shutil.which("dashboard")


def _stable_hub_port() -> int:
    """The port the one shared hub binds. A fixed default (8080) so the URL is the
    same every time; ``IX_DASH_HUB_PORT`` overrides. If it is taken by something
    that is not our hub, the launcher falls back to an ephemeral port."""
    pinned = os.environ.get("IX_DASH_HUB_PORT")
    if not pinned:
        return 8080
    try:
        return int(pinned)
    except ValueError:
        print(
            f"[ix-mcp] IX_DASH_HUB_PORT={pinned!r} is not an integer; using 8080",
            file=sys.stderr,
        )
        return 8080


def _bind_ip(host: str) -> str:
    """A concrete, non-wildcard IP literal to hand the `dashboard` binary. It
    parses ``host:port`` as a SocketAddr (IP only), so a hostname like
    ``localhost`` from ``IX_MCP_HOST`` would crash it on startup even though
    Python's bind accepts the name. A wildcard (``0.0.0.0``/``::``) is refused --
    it would serve the board (kernel data, outputs) on every NIC -- and mapped to
    loopback. Otherwise: pass IPs through; resolve a name; loopback if unresolvable."""
    if host in ("0.0.0.0", "::", ""):  # noqa: S104 -- refusing a wildcard bind, mapping it to loopback
        return "127.0.0.1"
    try:
        ipaddress.ip_address(host)
    except ValueError:
        try:
            return socket.gethostbyname(host)
        except OSError:
            return "127.0.0.1"
    return host


def _host_arg(host: str) -> str:
    """Bracket an IPv6 literal for use in a ``host:port`` string. The dashboard
    binary builds ``format!("{host}:{port}")`` and parses it as a SocketAddr,
    which requires ``[::1]:8080`` form; the same bracketing makes a valid URL
    authority. IPv4 and hostnames are returned unchanged (Python's own socket
    calls take the raw, unbracketed host)."""
    return f"[{host}]" if ":" in host else host


def _spawn_shared_hub() -> dict | None:
    """Start the one shared dashboard hub detached and record it in
    :func:`hub_state_path`. Returns its state dict, or ``None`` if the binary is
    missing or it never came up. Bound to the tailnet IP (or ``IX_MCP_HOST``, else
    loopback), never ``0.0.0.0`` -- see the bind rationale below; a tailnet peer
    joins via that IP. ``start_new_session`` so it outlives this launcher."""
    binp = _hub_bin()
    if not binp:
        print(
            "[ix-mcp] dashboard binary not found; build via nix (which bundles "
            "`dashboard`) or put it on PATH",
            file=sys.stderr,
        )
        return None

    # Bind like the data API (see `_serve`): this machine's tailnet IP when
    # Tailscale is up (the tailnet is the trust boundary, so a peer can join) or
    # IX_MCP_HOST, else loopback. Never a wildcard -- 0.0.0.0/:: would serve the
    # board (kernel namespace values, captured outputs) on every LAN/public NIC of
    # a multi-homed host -- so a wildcard request falls through to tailnet/loopback.
    requested = os.environ.get("IX_MCP_HOST") or ""
    if requested in ("0.0.0.0", "::"):  # noqa: S104 -- treat a wildcard request as "unspecified"
        requested = ""
    bind = _bind_ip(requested or _tailscale_ip() or "127.0.0.1")
    port = _stable_hub_port()
    # `--port 0` would tell the binary to pick an ephemeral port we cannot predict,
    # so the readiness probe below would wait on the wrong port and time out while
    # leaking a live hub. Resolve a concrete port up front instead.
    if port == 0:
        port = _free_port()
    elif not _bindable(bind, port):
        # Stable port busy on that interface: take an ephemeral one. If even that
        # fails the interface is unusable (a stopped-tailscale race) -> loopback.
        port = _free_port()
        if bind != "127.0.0.1" and not _bindable(bind, port):
            bind, port = "127.0.0.1", _free_port()

    # Advertise the actual bind IP, never the originally-requested host: after a
    # hostname-resolution or bindability fallback the request may not be where we
    # are listening, so a URL built from it would point users somewhere unreachable.
    # Bracket IPv6 for the binary's host:port and the URL authority.
    host_arg = _host_arg(bind)
    url = f"http://{host_arg}:{port}/"
    log = runtime_dir() / "hub.log"
    try:
        with log.open("ab") as logf:
            proc = subprocess.Popen(
                [binp, "--host", host_arg, "--port", str(port), "--record-ms", "0"],
                stdout=logf,
                stderr=subprocess.STDOUT,
                start_new_session=True,
            )
    except OSError as error:
        print(f"[ix-mcp] failed to start dashboard: {error}", file=sys.stderr)
        return None

    deadline = time.monotonic() + 8.0
    while time.monotonic() < deadline:
        # Check our own child FIRST: if it died (e.g. lost a bind race to another
        # launcher's hub), a `port_open` success would be the *winner's* listener,
        # and we would wrongly record hub.json with our dead pid. The flock in
        # `_dashboard` already serializes launches so this race should not occur,
        # but ordering the poll first keeps the readiness check honest regardless.
        if proc.poll() is not None:
            print(f"[ix-mcp] dashboard exited on startup; see {log}", file=sys.stderr)
            return None
        if port_open(port, bind):
            break
        time.sleep(0.1)
    else:
        # Never observed it listen: kill the detached child so a slow/failed start
        # does not leave an orphan hub running (the very pile-up this avoids).
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
        print(f"[ix-mcp] dashboard did not start listening in time; see {log}", file=sys.stderr)
        return None

    state = {"pid": proc.pid, "host": bind, "port": port, "url": url}
    hub_state_path().write_text(json.dumps(state))
    return state


def _dashboard(*, open_browser: bool = True) -> int:
    """Open the one shared dashboard, starting it if needed. Idempotent: a hub
    already running is reused (no second board), which is the whole point of the
    machine-wide singleton -- repeated runs never pile up dashboards."""
    state = ensure_shared_dashboard()
    if state is None:
        return 1
    url = state["url"]
    print(url)
    # Open the advertised URL (the hub binds the tailnet IP or loopback, so this
    # is reachable from this host too); only when attached to a terminal so an
    # embedder shelling out to `ix-mcp dashboard` does not pop a browser.
    if open_browser and sys.stdout.isatty():
        webbrowser.open(url)
    return 0


def ensure_shared_dashboard(*, open_browser: bool = False) -> dict | None:
    """Start or reuse the shared dashboard hub.

    Tool calls use this directly for first-use autostart, where stdout is the MCP
    protocol stream and must stay untouched. ``ix-mcp dashboard`` remains the
    user-facing CLI wrapper that prints the URL and applies the TTY browser-open
    policy.
    """
    # Serialize concurrent launches: without this, two `ix-mcp dashboard` runs can
    # both see no hub and both spawn one (TOCTOU between the check and the bind).
    # Holding an exclusive lock around check-or-spawn means the loser blocks, then
    # finds the winner's hub.json and reuses it.
    lock_path = runtime_dir() / "hub.lock"
    with lock_path.open("w") as lock:
        fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
        state = live_hub() or _spawn_shared_hub()
    if state is None:
        return None
    url = state["url"]
    if open_browser:
        webbrowser.open(url)
    return state


def _one_shot(code: str) -> int:
    """Run ``code`` on a fresh throwaway kernel and print stdout/stderr/result."""
    from jupyter_client.manager import start_new_kernel

    collected: dict[str, object] = {"result": None, "stdout": [], "stderr": []}

    def hook(msg: dict) -> None:
        msg_type = msg["msg_type"]
        content = msg["content"]
        if msg_type == "stream":
            collected["stdout" if content.get("name") == "stdout" else "stderr"].append(content.get("text", ""))  # type: ignore[union-attr]
        elif msg_type in ("execute_result", "display_data"):
            text = content.get("data", {}).get("text/plain")
            if text:
                collected["result"] = text
        elif msg_type == "error":
            collected["stderr"].append("\n".join(content.get("traceback", [])))  # type: ignore[union-attr]

    km, kc = start_new_kernel(kernel_name="python3")
    try:
        kc.execute_interactive(code, timeout=60, output_hook=hook, store_history=False)
    finally:
        kc.stop_channels()
        km.shutdown_kernel(now=True)

    stdout = "".join(collected["stdout"]).rstrip()  # type: ignore[arg-type]
    if stdout:
        print(f"stdout:\n{stdout}")
    stderr = _ANSI.sub("", "".join(collected["stderr"]).rstrip())  # type: ignore[arg-type]
    if stderr:
        print(f"stderr:\n{stderr}")
    if collected["result"] is not None:
        print(f"result:\n{collected['result']}")
    return 1 if collected["stderr"] else 0
