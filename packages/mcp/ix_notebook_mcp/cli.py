"""The ``ix-mcp`` command line.

  ix-mcp serve            run the notebook MCP server over stdio (the default
                          transport; what an MCP client launches)
  ix-mcp serve --http A   run it over streamable HTTP at A (host:port)
  ix-mcp lab              open the running server's JupyterLab co-edit URL
  ix-mcp eval EXPR        evaluate one expression on a throwaway kernel
  ix-mcp exec SRC         run statements on a throwaway kernel
"""

from __future__ import annotations

import argparse
import os
import re
import socket
import sys
import webbrowser
from pathlib import Path

from .config import Config, runtime_dir, set_config

_ANSI = re.compile(r"\x1b\[[0-9;]*m")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="ix-mcp", description="Notebook-first MCP server")
    parser.add_argument("--workdir", help="Directory notebooks live in (default: cwd)")
    sub = parser.add_subparsers(dest="command")

    serve = sub.add_parser("serve", help="Run the MCP server")
    serve.add_argument(
        "--http",
        nargs="?",
        const="127.0.0.1:8000",
        metavar="ADDR",
        help="Serve over streamable HTTP at host:port instead of stdio",
    )
    sub.add_parser("lab", help="Open the running server's JupyterLab co-edit URL")
    ev = sub.add_parser("eval", help="Evaluate one expression on a throwaway kernel")
    ev.add_argument("code")
    ex = sub.add_parser("exec", help="Run statements on a throwaway kernel")
    ex.add_argument("code")

    args = parser.parse_args(argv)
    command = args.command or "serve"
    if command == "serve":
        return _serve(args)
    if command == "lab":
        return _lab()
    if command in ("eval", "exec"):
        return _one_shot(args.code)
    parser.error(f"unknown command {command!r}")
    return 2


def _free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def _serve(args: argparse.Namespace) -> int:
    workdir = (Path(args.workdir).resolve() if args.workdir else Path.cwd())
    workdir.mkdir(parents=True, exist_ok=True)

    http = getattr(args, "http", None)
    stdin_fd = stdout_fd = None
    if http is None:
        # Hand the MCP protocol the real stdin/stdout, then point this process's
        # fd 0/1 somewhere harmless so the Jupyter Server's logging (and any
        # library `print`) can never corrupt the JSON-RPC stream.
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

    # Pick the Jupyter port up front so the lab URL is correct the moment a tool
    # asks for it (the extension's post-start hook runs before the socket binds,
    # so reading serverapp.port there would still see the unbound value).
    cfg = Config(
        workdir=workdir,
        jupyter_port=_free_port(),
        transport=transport,
        mcp_http_host=mcp_http_host,
        mcp_http_port=mcp_http_port,
        stdin_fd=stdin_fd,
        stdout_fd=stdout_fd,
    )
    set_config(cfg)

    from jupyter_server.serverapp import ServerApp

    ServerApp.launch_instance(
        argv=[
            f"--ServerApp.root_dir={cfg.workdir}",
            f"--ServerApp.ip={cfg.host}",
            f"--ServerApp.port={cfg.jupyter_port}",
            "--ServerApp.open_browser=False",
            f"--IdentityProvider.token={cfg.token}",
            "--ServerApp.log_level=WARN",
            # In-process extensions: ours (MCP + YDoc bridge) and jupyter_server_ydoc
            # (the server side of real-time collaboration). The browser
            # collaboration UI loads from the installed jupyter-collaboration lab
            # extension; the metapackage itself has no server loader.
            "--ServerApp.jpserver_extensions=ix_notebook_mcp=True",
            "--ServerApp.jpserver_extensions=jupyter_server_ydoc=True",
        ]
    )
    return 0


def _lab() -> int:
    url_file = runtime_dir() / "lab-url"
    if not url_file.exists():
        print("no running ix-mcp server found (start one with `ix-mcp serve`)", file=sys.stderr)
        return 1
    url = url_file.read_text().strip()
    print(url)
    webbrowser.open(url)
    return 0


def _one_shot(code: str) -> int:
    """Run ``code`` on a fresh kernel and print stdout/stderr/result, matching the
    historical ``ix-mcp eval``/``exec`` output shape (``result:\\n<repr>``). No
    Jupyter Server, no notebook: a quick scratch evaluation on the pinned
    interpreter's kernel."""
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
