"""Turn kernel IOPub messages into notebook outputs, and notebook outputs into
MCP content.

One direction (`output_from_message`) builds the nbformat output dicts that get
written into the notebook cell; the other (`to_mcp`) renders those for the agent,
with real image blocks for plots so a figure comes back as an image rather than a
base64 wall.
"""

from __future__ import annotations

import base64
import re
from typing import Any

import nbformat
from mcp import types as mcp_types

# Cap on a single text output returned to the agent, so a cell that prints a huge
# object cannot flood the agent's context. The notebook on disk keeps the full
# output; only the value handed back is clipped.
_MAX_TEXT_CHARS = 50_000
_MAX_IMAGES = 8
_ANSI = re.compile(r"\x1b\[[0-9;]*m")

# IOPub message types that carry cell output (as opposed to status/execute_input).
_OUTPUT_TYPES = frozenset({"stream", "execute_result", "display_data", "error"})

Content = mcp_types.TextContent | mcp_types.ImageContent


def output_from_message(msg: dict) -> dict | None:
    """Build an nbformat output for an IOPub message, or ``None`` if the message
    is not an output (status, execute_input, ...)."""
    if msg["msg_type"] not in _OUTPUT_TYPES:
        return None
    return nbformat.from_dict(nbformat.v4.output_from_msg(msg))


def to_mcp(outputs: list[dict]) -> list[Content]:
    """Render nbformat outputs as MCP content: text blocks plus image blocks for
    any PNG/JPEG."""
    content: list[Content] = []
    images = 0
    for output in outputs:
        kind = output.get("output_type")
        if kind == "stream":
            content.append(text(output.get("text", "")))
        elif kind in ("execute_result", "display_data"):
            data = output.get("data", {})
            for mime in ("image/png", "image/jpeg"):
                if images < _MAX_IMAGES and mime in data:
                    content.append(_image(mime, data[mime]))
                    images += 1
            if "text/plain" in data:
                content.append(text(data["text/plain"]))
            elif "text/html" in data and "image/png" not in data:
                content.append(text("[HTML output omitted; see the notebook]"))
        elif kind == "error":
            trace = "\n".join(output.get("traceback", [])) or (
                f"{output.get('ename', 'Error')}: {output.get('evalue', '')}"
            )
            content.append(text(trace))
    return content or [text("(no output)")]


def error_output(ename: str, evalue: str) -> dict:
    """A synthetic nbformat error output (used to record a timeout in the cell)."""
    return {"output_type": "error", "ename": ename, "evalue": evalue, "traceback": [f"{ename}: {evalue}"]}


def text(value: Any) -> mcp_types.TextContent:
    text = _ANSI.sub("", value if isinstance(value, str) else "".join(value))
    if len(text) > _MAX_TEXT_CHARS:
        text = f"{text[:_MAX_TEXT_CHARS]}\n... [truncated {len(text) - _MAX_TEXT_CHARS} chars; full output in the notebook]"
    return mcp_types.TextContent(type="text", text=text)


def _image(mime: str, data: Any) -> mcp_types.ImageContent:
    # nbformat stores image/png as a base64 string already; pass it through.
    encoded = data if isinstance(data, str) else base64.b64encode(bytes(data)).decode("ascii")
    return mcp_types.ImageContent(type="image", data=encoded.strip(), mimeType=mime)
