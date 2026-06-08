"""The ``ix-mcp`` command line.

  ix-mcp serve            run the MCP server over stdio (what a client launches)
  ix-mcp serve --http A   run it over streamable HTTP at A (host:port)
  ix-mcp eval EXPR        evaluate one expression on a throwaway kernel
  ix-mcp exec SRC         run statements on a throwaway kernel

`serve` starts ONE shared IPython kernel, an auto-started read-only dashboard
over the execution store, and the MCP transport, all on one event loop.
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
import webbrowser
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
    sub.add_parser("dashboard", help="Open the running server's dashboard URL")
    ev = sub.add_parser("eval", help="Evaluate one expression on a throwaway kernel")
    ev.add_argument("code")
    ex = sub.add_parser("exec", help="Run statements on a throwaway kernel")
    ex.add_argument("code")

    args = parser.parse_args(argv)
    command = args.command or "serve"
    if command == "serve":
        return _serve(args)
    if command == "dashboard":
        return _dashboard()
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


def _serve(args: argparse.Namespace) -> int:
    wd = getattr(args, "workdir", None)
    workdir = Path(wd).resolve() if wd else Path.cwd()
    workdir.mkdir(parents=True, exist_ok=True)

    bind_host = os.environ.get("IX_MCP_HOST") or _tailscale_ip() or "127.0.0.1"
    advertised_host = _advertised_host(bind_host)

    http = getattr(args, "http", None)
    stdin_fd = stdout_fd = None
    if http is None:
        # Hand the MCP protocol the real stdin/stdout, then point fd 0/1 at
        # /dev/null and stderr so nothing else can corrupt the JSON-RPC stream.
        stdin_fd = os.dup(0)
        stdout_fd = os.dup(1)
        os.dup2(2, 1)
        devnull = os.open(os.devnull, os.O_RDONLY)
        os.dup2(devnull, 0)
        os.close(devnull)
        mcp_http_host, mcp_http_port = "127.0.0.1", 8000
        transport = "stdio"
    else:
        transport = "http"
        host, _, port = http.partition(":")
        mcp_http_host, mcp_http_port = host or "127.0.0.1", int(port) if port else 8000

    dashboard_port = _dashboard_port()
    store_path = runtime_dir() / f"store-{dashboard_port}.db"
    # Fresh execution log per server: if this port was used by a prior server,
    # drop its database (and WAL sidecars) so the dashboard never shows stale runs.
    for suffix in ("", "-wal", "-shm"):
        (store_path.parent / (store_path.name + suffix)).unlink(missing_ok=True)

    cfg = Config(
        workdir=workdir,
        host=bind_host,
        advertised_host=advertised_host,
        dashboard_port=dashboard_port,
        store_path=store_path,
        transport=transport,
        mcp_http_host=mcp_http_host,
        mcp_http_port=mcp_http_port,
        stdin_fd=stdin_fd,
        stdout_fd=stdout_fd,
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

    asyncio.run(_run(cfg))
    return 0


async def _run(cfg: Config) -> None:
    from . import dashboard, tools, transport
    from .kernel import Kernel, set_kernel

    kernel = Kernel(cfg)
    await kernel.start()
    set_kernel(kernel)

    runner = await dashboard.start(cfg)
    url = cfg.dashboard_url()
    (runtime_dir() / "dashboard-url").write_text(url)
    # Bake the live URL into the MCP instructions before serving, so the client
    # gets it in the `initialize` response -- no tool call to discover it.
    tools.set_dashboard_url(url)
    print(f"[ix-mcp] dashboard (all running things + output): {url}", file=sys.stderr, flush=True)

    try:
        await transport.serve()
    finally:
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
