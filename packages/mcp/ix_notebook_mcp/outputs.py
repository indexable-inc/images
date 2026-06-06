"""Turn kernel IOPub messages into outputs for the agent and the store.

``output_from_message`` builds nbformat output dicts from raw IOPub messages;
``to_mcp`` renders those for the agent (real image blocks for plots, clipped
text). The kernel-side runtime also emits a structured summary under the
``application/x-ix-job+json`` mime type; :func:`job_summary` pulls it out.
"""

from __future__ import annotations

import base64
import re
from typing import Any

import nbformat
from mcp import types as mcp_types

_MAX_TEXT_CHARS = 50_000
_MAX_IMAGES = 8
_ANSI = re.compile(r"\x1b\[[0-9;]*m")

_OUTPUT_TYPES = frozenset({"stream", "execute_result", "display_data", "error"})

# The custom mime the kernel runtime uses to hand the server a job summary.
JOB_MIME = "application/x-ix-job+json"

Content = mcp_types.TextContent | mcp_types.ImageContent


def output_from_message(msg: dict) -> dict | None:
    if msg["msg_type"] not in _OUTPUT_TYPES:
        return None
    return nbformat.from_dict(nbformat.v4.output_from_msg(msg))


def job_summary(output: dict) -> dict | None:
    """Return the job summary carried by an nbformat output, or None."""
    if output.get("output_type") in ("execute_result", "display_data"):
        data = output.get("data", {})
        if JOB_MIME in data:
            return data[JOB_MIME]
    return None


def to_mcp(outputs: list[dict]) -> list[Content]:
    """Render nbformat outputs as MCP content, skipping the internal job summary."""
    content: list[Content] = []
    images = 0
    for output in outputs:
        kind = output.get("output_type")
        if kind == "stream":
            content.append(text(output.get("text", "")))
        elif kind in ("execute_result", "display_data"):
            data = output.get("data", {})
            if JOB_MIME in data:
                continue  # internal summary, surfaced separately
            for mime in ("image/png", "image/jpeg"):
                if images < _MAX_IMAGES and mime in data:
                    content.append(_image(mime, data[mime]))
                    images += 1
            if "text/plain" in data:
                content.append(text(data["text/plain"]))
            elif "text/html" in data and "image/png" not in data:
                content.append(text("[HTML output; see the dashboard]"))
        elif kind == "error":
            trace = "\n".join(output.get("traceback", [])) or (
                f"{output.get('ename', 'Error')}: {output.get('evalue', '')}"
            )
            content.append(text(trace))
    return content or [text("(no output)")]


def text(value: Any) -> mcp_types.TextContent:
    rendered = _ANSI.sub("", value if isinstance(value, str) else "".join(value))
    if len(rendered) > _MAX_TEXT_CHARS:
        rendered = f"{rendered[:_MAX_TEXT_CHARS]}\n... [truncated {len(rendered) - _MAX_TEXT_CHARS} chars]"
    return mcp_types.TextContent(type="text", text=rendered)


def _image(mime: str, data: Any) -> mcp_types.ImageContent:
    encoded = data if isinstance(data, str) else base64.b64encode(bytes(data)).decode("ascii")
    return mcp_types.ImageContent(type="image", data=encoded.strip(), mimeType=mime)
