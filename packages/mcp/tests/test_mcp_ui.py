"""The MCP Apps view mechanism (mcp_ui): the shape of what goes on the wire.

Covers the reusable module end to end against the stable 2026-01-26 MCP Apps
spec (github.com/modelcontextprotocol/ext-apps, specification/2026-01-26):

- the `ui://` viewer resource registers with the REQUIRED mimeType
  `text/html;profile=mcp-app` and serves a self-contained HTML document;
- `tool_meta` links a tool via `_meta.ui.resourceUri` (plus the deprecated
  flat key the reference example still ships);
- `ui_result` preserves the model-facing content blocks byte-for-byte and
  rides the human HTML in the result `_meta`, budget-clipped loudly;
- `html_fragments` pulls exactly the dashboard's `text/html` view out of a
  run's nbformat outputs;
- `embedded_html`/`job_payload` bake the same payload into the document the
  data API serves to the room's sandboxed iframe.
"""

from __future__ import annotations

import asyncio
import json
from pathlib import Path
from mcp import types as mcp_types
from mcp.server.fastmcp import FastMCP

from ix_notebook_mcp import mcp_ui, outputs


def text(value: str) -> mcp_types.TextContent:
    return mcp_types.TextContent(type="text", text=value)


# --------------------------------------------------------------------------- #
# tool_meta / register_viewer: the tool<->resource linkage the host discovers.
# --------------------------------------------------------------------------- #


def test_tool_meta_links_resource_via_meta_ui() -> None:
    meta = mcp_ui.tool_meta()
    assert meta["ui"]["resourceUri"] == mcp_ui.VIEWER_URI
    # The deprecated flat key rides along for pre-GA hosts, same value.
    assert meta["ui/resourceUri"] == mcp_ui.VIEWER_URI
    assert mcp_ui.VIEWER_URI.startswith("ui://")


def test_register_viewer_declares_ui_resource() -> None:
    server = FastMCP("test")
    meta = mcp_ui.register_viewer(server)
    assert meta == mcp_ui.tool_meta()

    listed = asyncio.run(server.list_resources())
    (resource,) = [r for r in listed if str(r.uri).startswith("ui://")]
    assert str(resource.uri) == mcp_ui.VIEWER_URI
    # Spec "Content Requirements": mimeType MUST be text/html;profile=mcp-app.
    assert resource.mimeType == mcp_ui.UI_MIME

    (contents,) = asyncio.run(server.read_resource(mcp_ui.VIEWER_URI))
    assert contents.mime_type == mcp_ui.UI_MIME
    assert isinstance(contents.content, str)
    document = contents.content
    assert document.lstrip().startswith("<!DOCTYPE html>")
    # The view implements the MCP Apps lifecycle by hand...
    for marker in (
        "ui/initialize",
        "ui/notifications/initialized",
        "ui/notifications/tool-result",
        "ui/notifications/tool-cancelled",
        "ui/notifications/size-changed",
        "ui/resource-teardown",
    ):
        assert marker in document, f"viewer must speak {marker}"
    # ...and is fully self-contained: no external origins, so the spec's
    # restrictive default CSP applies without any csp metadata.
    assert "http://" not in document
    assert "https://" not in document
    # The JS reads the same _meta key the Python side writes.
    assert mcp_ui.RESULT_META_KEY in document


def test_register_viewer_accepts_custom_document_and_uri() -> None:
    server = FastMCP("test")
    meta = mcp_ui.register_viewer(server, uri="ui://test/custom", html="<!DOCTYPE html><html></html>")
    assert meta["ui"]["resourceUri"] == "ui://test/custom"


# --------------------------------------------------------------------------- #
# ui_result: the CallToolResult shape.
# --------------------------------------------------------------------------- #


def test_ui_result_preserves_content_and_carries_html_in_meta() -> None:
    blocks = [text('{"job": "ab12", "status": "done"}'), text("model view")]
    result = mcp_ui.ui_result(blocks, fragments=["<table><tr><td>1</td></tr></table>"], title="count rows")

    assert isinstance(result, mcp_types.CallToolResult)
    # Model-facing blocks pass through untouched (same objects, same order).
    assert list(result.content) == blocks
    assert result.meta is not None
    view = result.meta[mcp_ui.RESULT_META_KEY]
    assert view["title"] == "count rows"
    assert view["html"] == ["<table><tr><td>1</td></tr></table>"]
    # And the wire alias is `_meta`, per spec.
    dumped = result.model_dump(mode="json", by_alias=True)
    assert mcp_ui.RESULT_META_KEY in dumped["_meta"]


def test_ui_result_without_fragments_has_no_meta() -> None:
    result = mcp_ui.ui_result([text("plain")])
    assert result.meta is None
    assert [b.text for b in result.content if isinstance(b, mcp_types.TextContent)] == ["plain"]


def test_ui_result_budget_clips_loudly_never_partially() -> None:
    big = "<div>" + "x" * 120 + "</div>"
    result = mcp_ui.ui_result([text("t")], fragments=[big, big, big], budget=len(big) * 2 + 1)
    view = (result.meta or {})[mcp_ui.RESULT_META_KEY]
    # Two whole fragments fit; the third is REPLACED by a visible notice --
    # never a truncated fragment (unbalanced markup), never silence.
    assert view["html"][:2] == [big, big]
    assert len(view["html"]) == 3
    assert "omitted" in view["html"][2]


# --------------------------------------------------------------------------- #
# html_fragments: exactly the dashboard's human view, out of nbformat outputs.
# --------------------------------------------------------------------------- #


def test_html_fragments_extracts_display_html_in_order() -> None:
    cell_outputs = [
        {"output_type": "stream", "text": "stdout noise"},
        {
            "output_type": "display_data",
            "data": {"text/html": "<p>first</p>", "text/plain": "first"},
        },
        # nbformat's list-of-lines form joins.
        {
            "output_type": "execute_result",
            "data": {"text/html": ["<p>", "second", "</p>"]},
        },
        # The internal job summary bundle never reaches the human view.
        {
            "output_type": "display_data",
            "data": {outputs.JOB_MIME: {"id": "ab12"}, "text/html": "<p>internal</p>"},
        },
        # A Result's split: text/html is the human side, IX_LLM_MIME the model's.
        {
            "output_type": "execute_result",
            "data": {
                "text/html": "<strong>human</strong>",
                outputs.IX_LLM_MIME: {"text": "model", "images": []},
            },
        },
        {"output_type": "display_data", "data": {"text/plain": "no html here"}},
    ]
    assert mcp_ui.html_fragments(cell_outputs) == [
        "<p>first</p>",
        "<p>second</p>",
        "<strong>human</strong>",
    ]


def test_html_fragments_skips_blank_html() -> None:
    assert mcp_ui.html_fragments([{"output_type": "display_data", "data": {"text/html": "   "}}]) == []


# --------------------------------------------------------------------------- #
# embedded_html / job_payload: the room's iframe document.
# --------------------------------------------------------------------------- #


def test_embedded_html_bakes_payload_and_escapes_script_close() -> None:
    payload = mcp_ui.result_payload(
        [text("</script><script>alert(1)</script>")],
        fragments=["<p>view</p>"],
    )
    document = mcp_ui.embedded_html(payload)
    # The payload text cannot close the carrier tag: every `<` is <-escaped.
    assert "</script><script>alert(1)" not in document
    assert '"\\u003c/script>' in document
    # And the baked JSON round-trips to the payload.
    start = document.index('id="ix-embedded-payload">') + len('id="ix-embedded-payload">')
    end = document.index("</script>", start)
    assert json.loads(document[start:end]) == payload
    # The served MCP resource keeps the null marker; only this doc replaces it.
    assert 'id="ix-embedded-payload">null<' in mcp_ui.viewer_html()


def test_job_payload_carries_header_error_and_fragments() -> None:
    job = {
        "id": "ab12",
        "name": "count rows per host",
        "status": "error",
        "started_at": 100.0,
        "ended_at": 101.5,
        "error": "ZeroDivisionError: division by zero",
        "output": "",
        "result": None,
        "outputs": [
            {"output_type": "display_data", "data": {"text/html": "<table></table>"}},
        ],
    }
    payload = mcp_ui.job_payload(job)
    header = json.loads(payload["content"][0]["text"])
    assert header == {"job": "ab12", "status": "error", "elapsed_s": 1.5}
    assert payload["content"][1]["text"] == "ZeroDivisionError: division by zero"
    view = payload["_meta"][mcp_ui.RESULT_META_KEY]
    assert view["title"] == "count rows per host"
    assert view["html"] == ["<table></table>"]


def test_job_payload_without_html_falls_back_to_result_text() -> None:
    job = {
        "id": "cd34",
        "name": "add numbers",
        "status": "done",
        "started_at": 1.0,
        "ended_at": 2.0,
        "error": None,
        "output": "",
        "result": "3",
        "outputs": [],
    }
    payload = mcp_ui.job_payload(job)
    assert payload["content"][-1]["text"] == "3"
    # No fragments -> the _meta still names the run so the frame has a title.
    assert payload["_meta"][mcp_ui.RESULT_META_KEY] == {"title": "add numbers"}


# --------------------------------------------------------------------------- #
# The opted-in tool surface: python_exec/pr_watch declare the viewer.
# --------------------------------------------------------------------------- #


def test_tools_declare_ui_resource_in_meta() -> None:
    from ix_notebook_mcp import tools

    by_name = {tool.name: tool for tool in tools.mcp._tool_manager.list_tools()}
    for name in ("python_exec", "pr_watch"):
        meta = by_name[name].meta
        assert meta is not None, f"{name} must opt into the UI viewer"
        assert meta["ui"]["resourceUri"] == mcp_ui.VIEWER_URI
    # `read` deliberately stays plain: its contract is full text to the model
    # with only a one-line note for the human.
    assert by_name["read"].meta is None


# --------------------------------------------------------------------------- #
# The data API route the room's sandboxed iframe loads.
# --------------------------------------------------------------------------- #


def test_api_job_ui_serves_embedded_view(tmp_path: Path) -> None:
    from aiohttp.test_utils import TestClient, TestServer

    from ix_notebook_mcp import dashboard, store
    from ix_notebook_mcp.config import Config

    db = tmp_path / "ui.db"
    conn = store.connect(db)
    store.start(conn, id="ab12", name="count rows", code="1+1", started_at=1.0)
    store.finish(
        conn,
        id="ab12",
        status="done",
        ended_at=2.0,
        output="",
        result="2",
        error=None,
        outputs=[{"output_type": "display_data", "data": {"text/html": "<table></table>"}}],
    )
    cfg = Config(workdir=tmp_path, store_path=db)

    async def run() -> None:
        client = TestClient(TestServer(dashboard.build_app(cfg, conn)))
        await client.start_server()
        try:
            resp = await client.get("/api/jobs/ab12/ui")
            assert resp.status == 200
            assert resp.content_type == "text/html"
            document = await resp.text()
            assert 'id="ix-embedded-payload">null<' not in document
            start = document.index('id="ix-embedded-payload">') + len('id="ix-embedded-payload">')
            payload = json.loads(document[start : document.index("</script>", start)])
            assert payload["_meta"][mcp_ui.RESULT_META_KEY]["html"] == ["<table></table>"]
            assert json.loads(payload["content"][0]["text"])["job"] == "ab12"
            # Unknown job: a loud 404, never a blank page.
            missing = await client.get("/api/jobs/nope/ui")
            assert missing.status == 404
        finally:
            await client.close()

    asyncio.run(run())


# --------------------------------------------------------------------------- #
# On the wire: a real (in-memory) MCP session sees the spec shapes end to end.
# --------------------------------------------------------------------------- #


def test_wire_tool_result_meta_and_resource_roundtrip() -> None:
    server = FastMCP("wire-test")
    ui_meta = mcp_ui.register_viewer(server)

    @server.tool(structured_output=False, meta=ui_meta)
    def show() -> mcp_types.CallToolResult:
        return mcp_ui.ui_result([text('{"status": "done"}')], fragments=["<p>hi</p>"], title="show")

    asyncio.run(_wire_roundtrip(server))


async def _wire_roundtrip(server: FastMCP) -> None:
    from mcp.shared.memory import create_connected_server_and_client_session

    async with create_connected_server_and_client_session(server._mcp_server) as session:
        tools = await session.list_tools()
        (tool,) = tools.tools
        # Host-side discovery: _meta.ui.resourceUri names the view to mount.
        assert tool.meta is not None
        assert tool.meta["ui"]["resourceUri"] == mcp_ui.VIEWER_URI

        read = await session.read_resource(mcp_types.AnyUrl(mcp_ui.VIEWER_URI))
        (contents,) = read.contents
        assert contents.mimeType == mcp_ui.UI_MIME
        assert isinstance(contents, mcp_types.TextResourceContents)
        assert "ui/notifications/tool-result" in contents.text

        result = await session.call_tool("show", {})
        assert not result.isError
        (block,) = result.content
        assert isinstance(block, mcp_types.TextContent)
        assert block.text == '{"status": "done"}'
        # The human view survives to the client verbatim: this is what the host
        # relays to the iframe as `ui/notifications/tool-result` params.
        assert result.meta is not None
        assert result.meta[mcp_ui.RESULT_META_KEY] == {"title": "show", "html": ["<p>hi</p>"]}

