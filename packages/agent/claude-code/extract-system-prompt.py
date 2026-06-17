"""Extract the stock Claude Code system prompt by capturing what the binary sends.

Claude Code natively honors `ANTHROPIC_BASE_URL`, so there is no need to defeat
the binary's packaging or do TLS interception: point the real upstream `claude`
at a throwaway localhost server, run it once in print mode, and read the exact
`system` blocks (and tool schemas) out of the request it transmits. The CLI does
the prompt assembly for us, interpolating its environment block, and hands over
the finished payload on a socket we own.

Two deliberate isolation choices keep the result the *stock* prompt:

  - It runs the unwrapped upstream binary (the package's `libexec` helper baked
    in as `--claude-binary`), never the Nix wrapper that bakes our house
    `--append-system-prompt-file`, MCP config, and settings.
  - It runs from a fresh temp HOME and an empty temp cwd, so no `~/.claude`
    settings, no project `CLAUDE.md`, and no git status leak into the capture.

The capture is print mode (`claude -p`), which uses the Agent SDK entrypoint, so
the identity line reads "You are a Claude agent, built on Anthropic's Claude
Agent SDK." rather than the interactive "You are Claude Code, ...". The body of
the prompt is otherwise the same; the interactive variant requires driving the
TUI.
"""

from __future__ import annotations

import argparse
import asyncio
import contextlib
import json
import os
import shutil
import sys
import tempfile
from typing import Any

# Baked at build time via the writePythonApplication `args` prefix
# (`--claude-binary <libexec helper>`); a user-supplied `--claude-binary` on the
# CLI appears later in argv and wins, so this is only the default.
DEFAULT_BINARY = "claude"


async def _read_http_request(reader: asyncio.StreamReader) -> tuple[str, bytes]:
    """Read one HTTP/1.1 request, returning (request_line, body)."""
    head = b""
    while b"\r\n\r\n" not in head:
        chunk = await reader.read(65536)
        if not chunk:
            break
        head += chunk
    raw_head, _, rest = head.partition(b"\r\n\r\n")
    lines = raw_head.decode("latin1").split("\r\n")
    request_line = lines[0] if lines else ""
    content_length = 0
    for line in lines[1:]:
        if line.lower().startswith("content-length:"):
            with contextlib.suppress(ValueError):
                content_length = int(line.split(":", 1)[1].strip())
    body = rest
    while len(body) < content_length:
        chunk = await reader.read(65536)
        if not chunk:
            break
        body += chunk
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
        request_line, body = await _read_http_request(reader)
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
        # Minimal valid Messages response so the CLI exits cleanly; by now we
        # already hold the request we came for, so its fate does not matter.
        payload = json.dumps(
            {
                "id": "msg_capture",
                "type": "message",
                "role": "assistant",
                "model": model,
                "content": [{"type": "text", "text": "ok"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 1, "output_tokens": 1},
            }
        ).encode()
        writer.write(
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n"
            b"Content-Length: %d\r\n\r\n" % len(payload) + payload
        )
        with contextlib.suppress(Exception):
            await writer.drain()
            writer.close()

    server = await asyncio.start_server(handle, "127.0.0.1", 0)
    port = server.sockets[0].getsockname()[1]
    serving = asyncio.create_task(server.serve_forever())

    home = tempfile.mkdtemp(prefix="claude-extract-home-")
    cwd = tempfile.mkdtemp(prefix="claude-extract-cwd-")
    env = {
        **os.environ,
        "HOME": home,
        "XDG_CONFIG_HOME": f"{home}/.config",
        "ANTHROPIC_BASE_URL": f"http://127.0.0.1:{port}",
        "ANTHROPIC_API_KEY": "sk-ant-extract-dummy",
        "DISABLE_TELEMETRY": "1",
        "DISABLE_ERROR_REPORTING": "1",
        "DISABLE_AUTOUPDATER": "1",
        "DISABLE_INSTALLATION_CHECKS": "1",
    }
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


def render_text(body: dict[str, Any], *, include_tools: bool) -> str:
    """Render the captured system blocks (and optionally tools) as readable text."""
    out: list[str] = []
    system = body.get("system", [])
    blocks = system if isinstance(system, list) else [{"text": system}]
    for i, block in enumerate(blocks):
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


def main() -> int:
    parser = argparse.ArgumentParser(
        prog="claude-code-extract-system-prompt",
        description="Capture and print the stock Claude Code system prompt.",
    )
    parser.add_argument(
        "--claude-binary",
        default=DEFAULT_BINARY,
        help="Path to the upstream claude binary to probe (default: baked libexec helper).",
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
        help="Seconds to wait for the request before giving up (default: 90).",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Print {model, system, tools} as JSON instead of readable text.",
    )
    parser.add_argument(
        "--raw",
        action="store_true",
        help="Print the entire captured request body as JSON.",
    )
    parser.add_argument(
        "--tools",
        action="store_true",
        help="In text mode, also print tool names and descriptions.",
    )
    parsed = parser.parse_args()

    try:
        body = asyncio.run(
            capture(
                parsed.claude_binary,
                model=parsed.model,
                prompt=parsed.prompt,
                timeout=parsed.timeout,
            )
        )
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
