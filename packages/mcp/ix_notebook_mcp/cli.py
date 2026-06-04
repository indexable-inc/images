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

# A bound socket on these binds to every interface, so the address itself is not
# a host anyone can dial; the lab URL must advertise a concrete reachable name.
_WILDCARD_HOSTS = {"0.0.0.0", "::"}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="ix-mcp", description="Notebook-first MCP server")
    sub = parser.add_subparsers(dest="command")

    serve = sub.add_parser("serve", help="Run the MCP server")
    # --workdir lives on `serve` (not the top-level parser) so the natural
    # `ix-mcp serve --workdir DIR` works; a top-level option would have to precede
    # the subcommand, which is a surprising ordering.
    serve.add_argument("--workdir", help="Directory notebooks live in (default: cwd)")
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


def _prepare_lab_config(jupyter_port: int) -> tuple[Path, bool]:
    """Materialize a writable JupyterLab config dir from the shipped assets so a
    fresh browser opens dark, Islands-colored, and in Berkeley Mono with no
    per-user setup.

    Returns ``(config_dir, has_custom_css)``. JupyterLab reads its custom CSS from
    ``{config_dir}/custom/custom.css`` and its default-settings overrides from
    the app settings dir; we point both at this per-server dir under the runtime
    dir, isolated from the user's ``~/.jupyter``. The assets are *copied* (not
    symlinked): Tornado's static handler refuses to follow a symlink that escapes
    its root into the Nix store, so a symlinked ``custom.css`` would 403.
    """
    assets = Path(__file__).resolve().parent / "jupyter"
    base = runtime_dir() / f"lab-{jupyter_port}"
    custom = base / "custom"
    settings = base / "lab-settings"
    custom.mkdir(parents=True, exist_ok=True)
    settings.mkdir(parents=True, exist_ok=True)

    overrides = assets / "overrides.json"
    if overrides.exists():
        shutil.copyfile(overrides, settings / "overrides.json")

    # islands.css is generated at build time (packages/mcp/default.nix); when
    # running straight from a source checkout it is absent, so custom CSS is
    # simply skipped rather than serving a 404 link.
    css = assets / "islands.css"
    has_css = css.exists()
    if has_css:
        shutil.copyfile(css, custom / "custom.css")
    return base, has_css


def _free_port() -> int:
    # Just reserves a free port number; the kernel gives port 0 an unused port.
    # The interface here is irrelevant: a number free on loopback is free for the
    # eventual bind too, so this stays 127.0.0.1 regardless of the final bind.
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def _tailscale_dns_name() -> str | None:
    """This node's MagicDNS name (e.g. ``host.tail<id>.ts.net``), or None.

    Best-effort: locating the binary and parsing `tailscale status --json` can
    fail many ways (no tailscale, not logged in, malformed output); every such
    case yields None rather than raising, so the caller can fall through.
    """
    tailscale = (
        shutil.which("tailscale")
        # macOS ships the CLI as a GUI-app shim outside PATH; Linux pkgs land here.
        or next((p for p in ("/usr/local/bin/tailscale", "/usr/bin/tailscale") if os.path.exists(p)), None)
    )
    if not tailscale:
        return None
    try:
        out = subprocess.run(
            [tailscale, "status", "--json"],
            capture_output=True,
            text=True,
            timeout=2,
            check=True,
        ).stdout
        name = json.loads(out).get("Self", {}).get("DNSName", "")
    except Exception:
        return None
    name = name.rstrip(".")
    return name or None


def _advertised_host(bind_host: str) -> str:
    """The host to put in the lab URL given the Jupyter bind address.

    An explicit ``IX_MCP_PUBLIC_HOST`` always wins. Otherwise a concrete bind
    host is already dialable and used as-is; only a wildcard bind needs a
    reachable name resolved for it.
    """
    public = os.environ.get("IX_MCP_PUBLIC_HOST")
    if public:
        return public
    if bind_host not in _WILDCARD_HOSTS:
        return bind_host
    # Wildcard bind: substitute a reachable name, best to worst. 127.0.0.1 last so
    # the URL is at least well-formed even if nothing better resolves.
    dns = _tailscale_dns_name()
    if dns:
        return dns
    fqdn = socket.getfqdn()
    if "." in fqdn and fqdn != "localhost":
        return fqdn
    return "127.0.0.1"


def _serve(args: argparse.Namespace) -> int:
    # `getattr`: a bare `ix-mcp` (no subcommand) defaults to serve but never ran
    # the serve subparser, so `args` has no `workdir` attribute then.
    wd = getattr(args, "workdir", None)
    workdir = Path(wd).resolve() if wd else Path.cwd()
    workdir.mkdir(parents=True, exist_ok=True)

    # Resolve both host knobs once here (Config is pure data): the bind address
    # Jupyter listens on, and the host advertised in the lab URL. Default bind is
    # loopback so the server is never exposed unless asked.
    bind_host = os.environ.get("IX_MCP_HOST", "127.0.0.1")
    advertised_host = _advertised_host(bind_host)

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
        host=bind_host,
        advertised_host=advertised_host,
        jupyter_port=_free_port(),
        transport=transport,
        mcp_http_host=mcp_http_host,
        mcp_http_port=mcp_http_port,
        stdin_fd=stdin_fd,
        stdout_fd=stdout_fd,
    )
    set_config(cfg)

    # A writable lab config dir (custom CSS + default-settings overrides) so the
    # co-edit browser opens dark, Islands-colored, and in Berkeley Mono with no
    # per-user setup. Isolated under the runtime dir, not the user's ~/.jupyter.
    lab_config_dir, has_custom_css = _prepare_lab_config(cfg.jupyter_port)
    # Run with this as the Jupyter config dir. This is deliberate isolation: the
    # co-edit server defines everything it needs through the flags below, so it
    # should not inherit (or be broken by) the user's ~/.jupyter server config,
    # and it keeps behavior reproducible across machines. It is also the only way
    # to relocate where custom CSS is served from ({config_dir}/custom): the
    # `static_custom_path` is derived from config_dir and is not itself settable,
    # and `--ServerApp.config_dir` is applied too late to move it. Set
    # unconditionally so the config surface does not change based on whether the
    # generated CSS happens to be present.
    os.environ["JUPYTER_CONFIG_DIR"] = str(lab_config_dir)

    from jupyter_server.serverapp import ServerApp

    ServerApp.launch_instance(
        argv=[
            f"--ServerApp.root_dir={cfg.workdir}",
            f"--ServerApp.ip={cfg.host}",
            f"--ServerApp.port={cfg.jupyter_port}",
            "--ServerApp.open_browser=False",
            # Empty token + empty password disables Jupyter auth entirely
            # (jupyter_server 2.x: auth_enabled becomes False, and
            # allow_unauthenticated_access defaults True), so the lab URL opens
            # straight in with no token to copy. Access is gated by reachability
            # instead: loopback by default, Tailscale-only when exposed. See the
            # Config bind-address comment for the security rationale.
            "--IdentityProvider.token=",
            "--ServerApp.log_level=WARN",
            # app_settings_dir holds overrides.json (default dark theme + editor
            # settings); custom CSS is served from {config_dir}/custom.
            f"--LabApp.app_settings_dir={lab_config_dir / 'lab-settings'}",
            *(
                ["--LabApp.custom_css=True"]
                if has_custom_css
                else []
            ),
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
