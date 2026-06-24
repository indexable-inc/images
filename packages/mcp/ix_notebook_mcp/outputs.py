"""Turn kernel IOPub messages into outputs for the agent and the store.

``output_from_message`` builds nbformat output dicts from raw IOPub messages;
``to_mcp`` renders those for the agent (real image blocks for plots, clipped
text). The kernel-side runtime also emits a structured summary under the
``application/x-ix-job+json`` mime type; :func:`job_summary` pulls it out.
"""

from __future__ import annotations

import base64
import json
import os
import re

import nbformat
from mcp import types as mcp_types

# Max chars of a single text block shown to the model per reply, overridable with
# ``IX_MCP_MAX_RESULT_CHARS`` (set it low to force aggressive paging, high to read
# more in one shot). The full output is never lost: it stays in the kernel as
# ``jobs['<id>']`` (see tools.python_exec), which pages it with
# tail/head/slice/grep/lines. An over-cap block is shown as a head+tail preview
# (see :func:`text`) so its shape is visible while the bulk is paged, not dumped.
try:
    MAX_TEXT_CHARS = max(500, int(os.environ.get("IX_MCP_MAX_RESULT_CHARS", "50000")))
except ValueError:
    MAX_TEXT_CHARS = 50_000
_MAX_IMAGES = 8

# Hard byte cap (decoded) on a single image block to the model. The kernel
# already fits Result images to this budget (IX_MCP_IMAGE_MAX_BYTES, see
# runtime._fit_image_bytes), so this is the final net catching any image that
# reaches the model UNfitted -- a raw ``display(fig)`` bundle that never went
# through a Result -- which is dropped with a short note here instead of dumped as
# megabytes of base64 (which floods the reply or is rejected by the host). Set
# ``IX_MCP_IMAGE_MAX_BYTES=0`` to disable.
try:
    MAX_IMAGE_BYTES = max(0, int(os.environ.get("IX_MCP_IMAGE_MAX_BYTES", "1000000")))
except ValueError:
    MAX_IMAGE_BYTES = 1_000_000
_ANSI = re.compile(r"\x1b\[[0-9;]*m")

_OUTPUT_TYPES = frozenset({"stream", "execute_result", "display_data", "error"})

# The custom mime the kernel runtime uses to hand the server a job summary.
JOB_MIME = "application/x-ix-job+json"

# The mime a Result uses to carry the model-facing view (text + images). The
# server unpacks it to real content blocks; it never reaches the dashboard.
IX_LLM_MIME = "application/x-ix-llm+json"

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
            if IX_LLM_MIME in data:
                # A Result's explicit model view: its text, then its images.
                spec = data[IX_LLM_MIME]
                if isinstance(spec, str):
                    try:
                        spec = json.loads(spec)
                    except json.JSONDecodeError:
                        spec = None
                if isinstance(spec, dict):
                    if spec.get("text"):
                        content.append(text(spec["text"]))
                    for img in spec.get("images", []):
                        if images < _MAX_IMAGES and isinstance(img, dict) and img.get("data"):
                            content.append(_image(img.get("mime", "image/png"), img["data"]))
                            images += 1
                continue
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


def text(value: str | list[str]) -> mcp_types.TextContent:
    rendered = _ANSI.sub("", value if isinstance(value, str) else "".join(value))
    if len(rendered) <= MAX_TEXT_CHARS:
        return mcp_types.TextContent(type="text", text=rendered)
    # Too large to return whole. Show a head AND a tail so the shape of the output
    # is visible (the end of a traceback, the last rows of a frame) rather than a
    # one-sided clip, and point at paging/filtering instead of dumping a wall. The
    # full run stays in the kernel as jobs['<id>'] (see the pager note in tools).
    head_n = MAX_TEXT_CHARS * 2 // 3
    tail_n = MAX_TEXT_CHARS - head_n
    omitted = len(rendered) - head_n - tail_n
    rendered = (
        f"{rendered[:head_n]}\n"
        f"... [output too large: {len(rendered)} chars, {omitted} omitted. Read the "
        f"full run with jobs['<id>'].grep('pattern') / .head(n) / .tail(n) / "
        f".slice(a, b), or narrow your query so it returns less.] ...\n"
        f"{rendered[-tail_n:]}"
    )
    return mcp_types.TextContent(type="text", text=rendered)


def _image(mime: str, data: str | bytes) -> Content:
    """One image as an MCP image block -- unless its decoded size exceeds
    ``MAX_IMAGE_BYTES``, in which case it is dropped with a short text note rather
    than flooding the reply with base64. ``data`` is base64 text or raw bytes."""
    if isinstance(data, str):
        try:
            raw = base64.b64decode(data, validate=True)
        except (ValueError, base64.binascii.Error):
            raw = None  # not decodable; pass the string through untouched below
    else:
        raw = bytes(data)
    if raw is not None and MAX_IMAGE_BYTES and len(raw) > MAX_IMAGE_BYTES:
        return text(
            f"[{mime} image dropped: {len(raw)} bytes exceeds the "
            f"{MAX_IMAGE_BYTES}-byte cap. Return it via Result(llm_images=[...]) -- "
            f"the kernel shrinks those to fit -- or raise IX_MCP_IMAGE_MAX_BYTES.]"
        )
    encoded = data.strip() if isinstance(data, str) else base64.b64encode(raw).decode("ascii")
    return mcp_types.ImageContent(type="image", data=encoded, mimeType=mime)
