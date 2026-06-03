"""Run code on a notebook's own kernel and turn the results into notebook
outputs.

The agent executes against the *same* kernel session a human's notebook uses, so
state (imports, variables) is shared, run a cell as the agent, inspect the
variable in the browser. We connect as an extra client on the kernel's ZeroMQ
sockets (the kernel's iopub is a PUB socket, so every client gets its own copy of
the messages; the agent never steals output from the browser).

The browser frontend only writes outputs for executions *it* started, so when
the agent runs a cell we collect the kernel's messages ourselves, build nbformat
outputs from them, and write those into the live YNotebook. That write is what
makes the agent's results show up in the human's tab and persist to ``.ipynb``.
"""

from __future__ import annotations

import asyncio
import base64
from typing import Any

import nbformat
from mcp import types as mcp_types

from .runtime import RUNTIME

# Drain trailing iopub messages for this long after the kernel reports idle.
# iopub and shell are separate sockets, so a late ``stream`` chunk can arrive
# just after the shell reply; this grace window catches it without hanging.
_IOPUB_GRACE_SECS = 0.2

# Cap on a single text output before it is truncated, so a cell that prints a
# huge object cannot flood the agent's context. The notebook on disk keeps the
# full output; only the value returned to the agent is clipped.
_MAX_TEXT_CHARS = 50_000

# Cap on images returned to the agent per execution.
_MAX_IMAGES = 8


async def ensure_kernel(rel_path: str) -> str:
    """Return the kernel id for ``rel_path``'s session, creating the session (and
    kernel) if needed. Keying the session on the path means the browser and the
    agent converge on one kernel for a given notebook."""
    session_manager = RUNTIME.serverapp.session_manager
    if await session_manager.session_exists(path=rel_path):
        existing = await session_manager.get_session(path=rel_path)
        return existing["kernel"]["id"]
    model = await session_manager.create_session(
        path=rel_path,
        name=rel_path,
        type="notebook",
        kernel_name="python3",
    )
    return model["kernel"]["id"]


async def restart_kernel(rel_path: str) -> None:
    serverapp = RUNTIME.serverapp
    kernel_id = await ensure_kernel(rel_path)
    await serverapp.kernel_manager.restart_kernel(kernel_id)


async def execute(rel_path: str, code: str, timeout: float) -> tuple[list[dict], int | None]:
    """Run ``code`` on the notebook's kernel; return (nbformat outputs, count).

    Raises :class:`TimeoutError` if the kernel does not finish within ``timeout``;
    the caller surfaces that and the kernel stays alive for the next call.
    """
    serverapp = RUNTIME.serverapp
    kernel_id = await ensure_kernel(rel_path)
    kernel = serverapp.kernel_manager.get_kernel(kernel_id)
    client = kernel.client()
    client.start_channels()
    try:
        await client.wait_for_ready(timeout=timeout)
        msg_id = client.execute(code)
        outputs, execution_count = await asyncio.wait_for(
            _collect(client, msg_id), timeout=timeout
        )
        return outputs, execution_count
    finally:
        client.stop_channels()


async def _collect(client: Any, msg_id: str) -> tuple[list[dict], int | None]:
    outputs: list[dict] = []
    execution_count: int | None = None
    idle = False
    while True:
        try:
            grace = _IOPUB_GRACE_SECS if idle else None
            msg = await client.get_iopub_msg(timeout=grace)
        except Exception:
            # Timed out waiting past idle: the trailing-message grace expired.
            if idle:
                break
            raise
        if msg.get("parent_header", {}).get("msg_id") != msg_id:
            continue
        msg_type = msg["msg_type"]
        content = msg["content"]
        if msg_type == "status":
            if content.get("execution_state") == "idle":
                idle = True
            continue
        if msg_type == "execute_input":
            execution_count = content.get("execution_count")
            continue
        if msg_type in ("stream", "execute_result", "display_data", "error"):
            outputs.append(_strip(nbformat.v4.output_from_msg(msg)))
            if msg_type == "execute_result":
                execution_count = content.get("execution_count", execution_count)
    return outputs, execution_count


def _strip(output: dict) -> dict:
    """nbformat ``NotebookNode`` -> plain dict, for JSON/YDoc round-tripping."""
    return nbformat.from_dict(output)


def write_outputs(ynb: Any, index: int, outputs: list[dict], execution_count: int | None) -> None:
    """Write execution results into the live notebook cell, so collaborators see
    them and they persist to disk."""
    cell = ynb.get_cell(index)
    cell["outputs"] = outputs
    cell["execution_count"] = execution_count
    ynb.set_cell(index, cell)


def outputs_to_mcp(outputs: list[dict]) -> list[mcp_types.TextContent | mcp_types.ImageContent]:
    """Render nbformat outputs as MCP content: text blocks plus real image blocks
    for any ``image/png``/``image/jpeg`` (so a matplotlib plot comes back as an
    image the agent can see, not a base64 wall)."""
    content: list[Any] = []
    images = 0
    for output in outputs:
        kind = output.get("output_type")
        if kind == "stream":
            content.append(_text(output.get("text", "")))
        elif kind in ("execute_result", "display_data"):
            data = output.get("data", {})
            for mime in ("image/png", "image/jpeg"):
                if images < _MAX_IMAGES and mime in data:
                    content.append(_image(mime, data[mime]))
                    images += 1
            text = data.get("text/plain")
            if text:
                content.append(_text(text))
            elif "text/html" in data and "image/png" not in data:
                content.append(_text("[HTML output omitted; see the notebook]"))
        elif kind == "error":
            trace = "\n".join(output.get("traceback", [])) or (
                f"{output.get('ename', 'Error')}: {output.get('evalue', '')}"
            )
            content.append(_text(_strip_ansi(trace)))
    if not content:
        content.append(_text("(no output)"))
    return content


def _text(value: Any) -> mcp_types.TextContent:
    text = value if isinstance(value, str) else "".join(value)
    text = _strip_ansi(text)
    if len(text) > _MAX_TEXT_CHARS:
        text = f"{text[:_MAX_TEXT_CHARS]}\n... [truncated {len(text) - _MAX_TEXT_CHARS} chars; full output in the notebook]"
    return mcp_types.TextContent(type="text", text=text)


def _image(mime: str, data: Any) -> mcp_types.ImageContent:
    # nbformat stores image/png as a base64 string already; pass it through.
    encoded = data if isinstance(data, str) else base64.b64encode(bytes(data)).decode("ascii")
    return mcp_types.ImageContent(type="image", data=encoded.strip(), mimeType=mime)


def _strip_ansi(text: str) -> str:
    import re

    return re.sub(r"\x1b\[[0-9;]*m", "", text)
