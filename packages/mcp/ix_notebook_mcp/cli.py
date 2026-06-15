"""The ``ix-mcp`` command line.

  ix-mcp serve                 run the MCP server over stdio (what a client launches)
  ix-mcp serve --http A        run it over streamable HTTP at A (host:port)
  ix-mcp serve --session F     same, but F is a persistent session file (see below)
  ix-mcp notebook [F]          run the notebook engine alone (kernel + dashboard, no MCP)
  ix-mcp eval EXPR             evaluate one expression on a throwaway kernel
  ix-mcp exec SRC              run statements on a throwaway kernel

`serve` starts ONE shared IPython kernel, an auto-started read-only dashboard
over the execution store, and the MCP transport, all on one event loop.
`notebook` is the engine without the MCP surface: the same kernel, store, and
dashboard, driven only by what is already in the session file and the humans
watching it.

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
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import time
import webbrowser
from dataclasses import replace
from pathlib import Path

from .config import Config, runtime_dir, set_config

_ANSI = re.compile(r"\x1b\[[0-9;]*m")
_WILDCARD_HOSTS = {"0.0.0.0", "::"}


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
    sub.add_parser("dashboard", help="Open the running server's dashboard URL")
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
        return _dashboard()
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


def _tailscale_status() -> dict | None:
    tailscale = shutil.which("tailscale") or next(
        (p for p in ("/usr/local/bin/tailscale", "/usr/bin/tailscale") if os.path.exists(p)), None
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
        if isinstance(ip, str) and "." in ip and ":" not in ip:
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
    exists=os.path.exists,
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
    if path and os.path.exists(path):
        return Path(path).read_text().strip()
    return None


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


def _serve(args: argparse.Namespace, *, engine_only: bool = False) -> int:
    wd = getattr(args, "workdir", None)
    workdir = Path(wd).resolve() if wd else Path.cwd()
    workdir.mkdir(parents=True, exist_ok=True)

    bind_host = os.environ.get("IX_MCP_HOST") or _tailscale_ip() or "127.0.0.1"
    advertised_host = _advertised_host(bind_host)

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

    dashboard_port = _dashboard_port()
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

    cfg = Config(
        workdir=workdir,
        host=bind_host,
        advertised_host=advertised_host,
        dashboard_port=dashboard_port,
        store_path=store_path,
        session_path=session_path,
        session_resume=session_resume,
        transport=transport,
        mcp_http_host=mcp_http_host,
        mcp_http_port=mcp_http_port,
        stdin_fd=stdin_fd,
        stdout_fd=stdout_fd,
        exec_token=_exec_token(),
        exec_trust_network=_exec_trust_network(),
    )
    set_config(cfg)

    # The kernel inherits this process's env, so set the store path (the runtime
    # writes there) and the private IPYTHONDIR (so the runtime startup runs)
    # before the kernel starts.
    os.environ["IX_MCP_STORE"] = str(store_path)
    # Surface the dashboard URL to the kernel so `DASHBOARD_URL` is one lookup
    # away (the agent should not have to spelunk the runtime dir to find it).
    os.environ["IX_MCP_DASHBOARD_URL"] = cfg.dashboard_url()
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


async def _run(cfg: Config) -> None:
    from . import dashboard, pane_bridge, tools, transport
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

    runner, bound_host = await dashboard.start(cfg)
    if bound_host != cfg.host:
        cfg = replace(cfg, host=bound_host, advertised_host=_advertised_host(bound_host))
        set_config(cfg)
    # Also publish every run/resource/namespace as Loro panes to the shared
    # dashboard hub (a separate `dashboard` process aggregates all producers).
    # Best-effort: if the producer socket cannot bind, this is silent and the
    # read-only API dashboard still works.
    bridge_task = asyncio.ensure_future(pane_bridge.run(cfg.store_path))
    url = cfg.dashboard_url()
    (runtime_dir() / "dashboard-url").write_text(url)
    # Bake the live URL into the MCP instructions before serving, so the client
    # gets it in the `initialize` response -- no tool call to discover it.
    tools.set_dashboard_url(url)
    print(f"[ix-mcp] dashboard (all running things + output): {url}", file=sys.stderr, flush=True)
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
        if restore_task is not None and not restore_task.done():
            restore_task.cancel()
        if cfg.session_path is not None:
            # Final checkpoint so the last cells' state reopens instantly even
            # when the debounced checkpoint had not fired yet.
            await kernel.snapshot_session()
        await runner.cleanup()
        await kernel.shutdown()


def _dashboard() -> int:
    url_file = runtime_dir() / "dashboard-url"
    if not url_file.exists():
        print("no running ix-mcp server found (start one with `ix-mcp serve`)", file=sys.stderr)
        return 1
    url = url_file.read_text().strip()
    print(url)
    webbrowser.open(url)
    return 0


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
