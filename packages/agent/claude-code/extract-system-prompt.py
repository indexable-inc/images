"""Capture the Claude Code system prompt by reading what the binary sends.

Claude Code natively honors `ANTHROPIC_BASE_URL`, so there is no need to defeat
the binary's packaging or do TLS interception: point the real `claude` binary at
a throwaway localhost server, run it once in print mode, and read the exact
`system` blocks (and tool schemas) out of the request it transmits. The CLI does
the prompt assembly for us, interpolating its environment block, and hands over
the finished payload on a socket we own.

It can probe two binaries and compare them (`--mode`):

  - stock: the unwrapped upstream binary (the package's `libexec` helper), which
    is the plain download with no house overrides.
  - wrapped: this package's launcher (`bin/claude`), which bakes the house
    --system-prompt-file (a full replacement of the stock prompt), --mcp-config,
    and --settings.
  - diff: a unified diff of the two, plus which tools the wrapper adds/removes.

Every capture runs from a fresh temp HOME and an empty temp cwd, so no
`~/.claude` settings, no project `CLAUDE.md`, and no git status leak in; the
only difference between stock and wrapped is the wrapper's own baked flags.

The capture is print mode (`claude -p`), which uses the Agent SDK entrypoint, so
the stock identity line reads "You are a Claude agent, built on Anthropic's
Claude Agent SDK." rather than the interactive "You are Claude Code, ...". The
interactive variant requires driving the TUI.
"""

from __future__ import annotations

import argparse
import asyncio
import contextlib
import difflib
import json
import os
import shutil
import sys
import tempfile
from typing import Any

# Baked at build time via the writePythonApplication `args` prefix
# (`--stock-binary <libexec helper>` / `--wrapped-binary <bin/claude>`); a
# user-supplied value on the CLI lands later in argv and wins, so these are only
# defaults. "stock" is the unwrapped upstream binary; "wrapped" is the Nix
# launcher that bakes this package's --system-prompt-file / --mcp-config /
# --settings overrides.
DEFAULT_STOCK_BINARY = "claude"
DEFAULT_WRAPPED_BINARY = "claude"


async def _read_http_request(reader: asyncio.StreamReader) -> tuple[str, bytes]:
    """Read exactly one HTTP/1.1 request, returning (request_line, body).

    Consumes precisely the headers and Content-Length body from the stream and
    leaves anything after untouched, so a keep-alive connection that pipelines a
    second request (e.g. count_tokens then messages on one socket) reads cleanly
    on the next call. Returns ("", b"") at end of stream.
    """
    try:
        raw_head = await reader.readuntil(b"\r\n\r\n")
    except (asyncio.IncompleteReadError, asyncio.LimitOverrunError):
        return "", b""
    lines = raw_head.decode("latin1").split("\r\n")
    request_line = lines[0] if lines else ""
    content_length = 0
    for line in lines[1:]:
        if line.lower().startswith("content-length:"):
            with contextlib.suppress(ValueError):
                content_length = int(line.split(":", 1)[1].strip())
    body = b""
    if content_length:
        try:
            body = await reader.readexactly(content_length)
        except asyncio.IncompleteReadError as err:
            body = err.partial
    return request_line, body


async def capture(
    binary: str,
    *,
    model: str,
    prompt: str,
    timeout: float,
) -> dict[str, Any]:
    """Run `binary` against a one-shot capture server; return the Messages body.

    Raises RuntimeError if the binary never sends a `/v1/messages` request.
    """
    captured: list[dict[str, Any]] = []
    done = asyncio.Event()

    async def handle(reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        # Serve every request on the connection until EOF, so a keep-alive CLI
        # that reuses one socket for count_tokens then messages is still caught.
        try:
            while True:
                request_line, body = await _read_http_request(reader)
                if not request_line:
                    break
                is_messages = (
                    request_line.startswith("POST ")
                    and "/v1/messages" in request_line
                    and "count_tokens" not in request_line
                )
                if is_messages and body and not captured:
                    with contextlib.suppress(json.JSONDecodeError):
                        parsed: dict[str, Any] = json.loads(body)
                        captured.append(parsed)
                        done.set()
                # Reply with the shape each endpoint expects so the CLI doesn't
                # error or retry before it gets to the messages request we want:
                # count_tokens wants {input_tokens}, messages wants a message. By
                # the time we hold our capture, the response's fate is moot.
                if "count_tokens" in request_line:
                    reply: dict[str, Any] = {"input_tokens": 1}
                else:
                    reply = {
                        "id": "msg_capture",
                        "type": "message",
                        "role": "assistant",
                        "model": model,
                        "content": [{"type": "text", "text": "ok"}],
                        "stop_reason": "end_turn",
                        "usage": {"input_tokens": 1, "output_tokens": 1},
                    }
                payload = json.dumps(reply).encode()
                writer.write(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n"
                    b"Content-Length: %d\r\n\r\n" % len(payload) + payload
                )
                await writer.drain()
        except (ConnectionError, asyncio.IncompleteReadError):
            # The CLI hung up mid-exchange (it got its response, or we killed it
            # after capture). Expected teardown, nothing to handle.
            pass
        finally:
            with contextlib.suppress(Exception):
                writer.close()

    # Raise the per-connection buffer limit well above a captured request body
    # (prompt + ~20 tool schemas runs tens of KiB) so readuntil never overruns.
    server = await asyncio.start_server(handle, "127.0.0.1", 0, limit=4 * 1024 * 1024)
    port = server.sockets[0].getsockname()[1]
    serving = asyncio.create_task(server.serve_forever())

    home = tempfile.mkdtemp(prefix="claude-extract-home-")
    cwd = tempfile.mkdtemp(prefix="claude-extract-cwd-")
    # Build a hermetic child env so the capture reflects the binary, not the
    # maintainer's shell. Drop every ANTHROPIC_*/CLAUDE_* var (model, token,
    # context-window, thinking-budget knobs that would skew what is sent) and any
    # *_PROXY (which would route the loopback request away from our server), then
    # set back only what we control. The wrapped launcher gets its own settings
    # from baked flags / IX_LAUNCH_SPEC, not from these, so this is safe for both.
    def _hermetic(key: str) -> bool:
        upper = key.upper()
        if upper in {"HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY"}:
            return False
        return not (upper.startswith(("ANTHROPIC_", "CLAUDE_")))

    env = {k: v for k, v in os.environ.items() if _hermetic(k)}
    env.update(
        {
            "HOME": home,
            "XDG_CONFIG_HOME": f"{home}/.config",
            "ANTHROPIC_BASE_URL": f"http://127.0.0.1:{port}",
            "ANTHROPIC_API_KEY": "sk-ant-extract-dummy",
            "NO_PROXY": "127.0.0.1,localhost",
            "no_proxy": "127.0.0.1,localhost",
            "DISABLE_TELEMETRY": "1",
            "DISABLE_ERROR_REPORTING": "1",
            "DISABLE_AUTOUPDATER": "1",
            "DISABLE_INSTALLATION_CHECKS": "1",
        }
    )
    proc = await asyncio.create_subprocess_exec(
        binary,
        "-p",
        prompt,
        "--model",
        model,
        cwd=cwd,
        env=env,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.STDOUT,
    )
    try:
        # Return as soon as the request is captured (done) OR the child exits
        # early without sending one (comm). `comm` exists only to detect that
        # early exit so we don't wait the full timeout; the process is reaped in
        # the `finally` below, so the cancelled task here leaks nothing.
        comm = asyncio.create_task(proc.communicate())
        _, pending = await asyncio.wait(
            {comm, asyncio.create_task(done.wait())},
            timeout=timeout,
            return_when=asyncio.FIRST_COMPLETED,
        )
        for task in pending:
            task.cancel()
    finally:
        if proc.returncode is None:
            proc.kill()
            with contextlib.suppress(Exception):
                await proc.wait()
        serving.cancel()
        server.close()
        with contextlib.suppress(Exception):
            await server.wait_closed()
        shutil.rmtree(home, ignore_errors=True)
        shutil.rmtree(cwd, ignore_errors=True)

    if not captured:
        raise RuntimeError(
            f"{binary} sent no /v1/messages request within {timeout:.0f}s; "
            "the binary may have failed to start (check it runs standalone)."
        )
    return captured[0]


def _blocks(body: dict[str, Any]) -> list[dict[str, Any]]:
    system = body.get("system", [])
    return system if isinstance(system, list) else [{"text": system}]


def system_text(body: dict[str, Any], *, skip_metadata: bool = False) -> str:
    """Concatenate the system blocks into one string.

    With skip_metadata, drop the leading `x-anthropic-billing-header` block,
    which carries a per-run nonce that is noise in a diff.
    """
    out: list[str] = []
    for block in _blocks(body):
        text = str(block.get("text", ""))
        if skip_metadata and text.startswith("x-anthropic-billing-header:"):
            continue
        out.append(text)
    return "\n".join(out)


def tool_names(body: dict[str, Any]) -> list[str]:
    return [t.get("name", "?") for t in body.get("tools", [])]


def render_text(body: dict[str, Any], *, include_tools: bool) -> str:
    """Render the captured system blocks (and optionally tools) as readable text."""
    out: list[str] = []
    for i, block in enumerate(_blocks(body)):
        cache = block.get("cache_control")
        out.append(f"===== system block {i} (cache_control={cache}) =====")
        out.append(str(block.get("text", "")))
        out.append("")
    if include_tools:
        tools = body.get("tools", [])
        out.append(f"===== tools ({len(tools)}) =====")
        for tool in tools:
            name = tool.get("name", "?")
            desc = (tool.get("description") or "").strip()
            out.append(f"\n## {name}\n")
            out.append(desc)
    return "\n".join(out).rstrip() + "\n"


def render_diff(stock: dict[str, Any], wrapped: dict[str, Any]) -> str:
    """Unified diff of stock vs wrapped system prompt, plus a tool-set summary."""
    out: list[str] = []
    diff = difflib.unified_diff(
        system_text(stock, skip_metadata=True).splitlines(),
        system_text(wrapped, skip_metadata=True).splitlines(),
        fromfile="stock (upstream)",
        tofile="wrapped (house overrides)",
        lineterm="",
    )
    out.extend(diff)
    if not out:
        out.append("(system prompts are identical)")

    stock_tools, wrapped_tools = set(tool_names(stock)), set(tool_names(wrapped))
    added = sorted(wrapped_tools - stock_tools)
    removed = sorted(stock_tools - wrapped_tools)
    out.append("")
    out.append("===== tools =====")
    out.append(f"stock: {len(stock_tools)}  |  wrapped: {len(wrapped_tools)}")
    out.append(f"added by wrapper:   {', '.join(added) or '(none)'}")
    out.append(f"removed by wrapper: {', '.join(removed) or '(none)'}")
    if any(t.startswith("mcp__") for t in added):
        # MCP tools only appear if the baked servers actually connected; the exa
        # server is HTTP to the internet, so an offline run under-reports them.
        out.append("note: mcp__* tools depend on those servers connecting (exa needs network).")
    return "\n".join(out) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(
        prog="claude-code-extract-system-prompt",
        description="Capture the Claude Code system prompt and tools as actually sent.",
    )
    parser.add_argument(
        "--mode",
        choices=("stock", "wrapped", "diff"),
        default="stock",
        help=(
            "stock: unwrapped upstream prompt (default). wrapped: this package's "
            "binary with its --system-prompt-file / MCP / settings overrides. "
            "diff: a unified diff of stock vs wrapped."
        ),
    )
    parser.add_argument(
        "--stock-binary",
        default=DEFAULT_STOCK_BINARY,
        help="Unwrapped upstream binary (default: baked libexec helper).",
    )
    parser.add_argument(
        "--wrapped-binary",
        default=DEFAULT_WRAPPED_BINARY,
        help="Wrapped launcher with house overrides (default: baked bin/claude).",
    )
    parser.add_argument(
        "--model",
        default="claude-opus-4-8",
        help="Model id passed to `claude -p` (default: claude-opus-4-8).",
    )
    parser.add_argument(
        "--prompt",
        default="hi",
        help="Throwaway user message used to trigger one request (default: 'hi').",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=90.0,
        help="Seconds to wait for each request before giving up (default: 90).",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Print {model, system, tools} as JSON (stock/wrapped modes).",
    )
    parser.add_argument(
        "--raw",
        action="store_true",
        help="Print the entire captured request body as JSON (stock/wrapped modes).",
    )
    parser.add_argument(
        "--tools",
        action="store_true",
        help="In text mode, also print tool names and descriptions (stock/wrapped modes).",
    )
    parsed = parser.parse_args()

    def grab(binary: str) -> dict[str, Any]:
        return asyncio.run(
            capture(binary, model=parsed.model, prompt=parsed.prompt, timeout=parsed.timeout)
        )

    try:
        if parsed.mode == "diff":
            sys.stdout.write(render_diff(grab(parsed.stock_binary), grab(parsed.wrapped_binary)))
            return 0
        binary = parsed.wrapped_binary if parsed.mode == "wrapped" else parsed.stock_binary
        body = grab(binary)
    except RuntimeError as err:
        print(f"error: {err}", file=sys.stderr)
        return 1

    if parsed.raw:
        print(json.dumps(body, indent=2, ensure_ascii=False))
    elif parsed.json:
        subset = {key: body[key] for key in ("model", "system", "tools") if key in body}
        print(json.dumps(subset, indent=2, ensure_ascii=False))
    else:
        sys.stdout.write(render_text(body, include_tools=parsed.tools))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
