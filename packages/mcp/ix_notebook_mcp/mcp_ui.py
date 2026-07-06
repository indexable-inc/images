"""Interactive HTML views for tool results, over MCP Apps (SEP-1865).

The dashboard and the room already render a run's rich HTML (the ``text/html``
mime the kernel displays); this module carries the SAME view back to the MCP
client, so a host that supports the MCP Apps extension (claude.ai, Claude
Desktop, ...) renders it inline in the chat. The mechanism, per the stable
2026-01-26 spec (https://github.com/modelcontextprotocol/ext-apps
``specification/2026-01-26/apps.mdx``):

- The server declares a **UI resource** under the ``ui://`` scheme whose
  mimeType is ``text/html;profile=mcp-app`` (:func:`register_viewer`).
- A tool opts in by naming that resource in its ``_meta.ui.resourceUri``
  (:func:`tool_meta`, passed as ``@mcp.tool(meta=...)``).
- When the tool is called, the host mounts the resource's HTML in a sandboxed
  iframe and hands it the tool's ``CallToolResult`` via the
  ``ui/notifications/tool-result`` notification (after the view's
  ``ui/initialize`` handshake). The view here implements that handshake by hand
  -- the spec's "you don't need an SDK" path -- so the document is fully
  self-contained: no CDN, no network, which also means the spec's restrictive
  default CSP needs no loosening (no ``csp`` metadata to declare).

The model-facing reply is UNCHANGED: :func:`ui_result` wraps the exact content
blocks a tool already returns in a ``CallToolResult`` and rides the rich HTML
fragments (the ones the dashboard shows) in the result's ``_meta`` under
:data:`RESULT_META_KEY`. ``_meta`` is host/view plumbing, not model context, so
the human's view never costs tokens -- the same split ``Result(user_html,
llm_result)`` already makes for the dashboard. A host that ignores the
extension ignores the metadata too and the tool behaves exactly as before.

The same document renders OUTSIDE an MCP host: :func:`embedded_html` bakes a
result payload into the template (read from an inline ``application/json``
script tag instead of ``ui/notifications/tool-result``), which is what the
data API's ``/api/jobs/{id}/ui`` route serves and the room mounts in a
sandboxed iframe. One template, two transports.

Reuse: any tool on any FastMCP server opts in with two calls --
``meta = mcp_ui.register_viewer(server)`` once, then ``@server.tool(meta=meta)``
and ``return mcp_ui.ui_result(content, fragments=...)``.
"""

from __future__ import annotations

import json
import os
from collections.abc import Mapping, Sequence
from typing import TYPE_CHECKING, Any

from mcp import types as mcp_types

if TYPE_CHECKING:
    from mcp.server.fastmcp import FastMCP

# The mimeType the spec REQUIRES for an MCP Apps HTML view ("Content
# Requirements": mimeType MUST be `text/html;profile=mcp-app`).
UI_MIME = "text/html;profile=mcp-app"

# The one shared viewer resource this server declares. Every opted-in tool
# points at it; the per-call data arrives via the tool-result notification, so
# the template is static and hosts may prefetch/cache it.
VIEWER_URI = "ui://ix-mcp/tool-result-viewer"

# Namespaced key for this server's view payload inside a CallToolResult's
# `_meta`. The spec forwards result `_meta` to the view verbatim in
# `ui/notifications/tool-result` (its params ARE the CallToolResult); a
# reverse-DNS-ish prefix keeps it clear of spec-owned keys.
RESULT_META_KEY = "io.indexable.ix/ui"

# Total budget (chars) for the HTML fragments carried on one result's `_meta`.
# claude.ai diverts tool results past ~150k chars out of the inline
# conversation (the app then never hydrates), and the model-facing text is
# already capped at outputs.MAX_TEXT_CHARS (50k default), so the human view
# gets a similar slice. Truncation is surfaced IN the view (a visible notice
# fragment), never silent.
try:
    HTML_BUDGET = max(1_000, int(os.environ.get("IX_MCP_UI_HTML_MAX_CHARS", "60000")))
except ValueError:
    HTML_BUDGET = 60_000

# Marker the template reads an embedded payload from (see `embedded_html`).
# In the served MCP resource it stays `null`: the view then waits for
# `ui/notifications/tool-result` instead.
_EMBED_MARKER = '<script type="application/json" id="ix-embedded-payload">null</script>'


def tool_meta(resource_uri: str = VIEWER_URI) -> dict[str, Any]:
    """The ``meta=`` dict that links a tool to its UI resource.

    ``ui.resourceUri`` is the spec's key ("Resource Discovery": tools are
    associated with UI resources through the ``_meta.ui`` field). The flat
    ``ui/resourceUri`` twin is the deprecated pre-GA spelling, kept alongside
    exactly as the reference Python example (ext-apps ``examples/qr-server``)
    does, for hosts that still read it.
    """
    return {"ui": {"resourceUri": resource_uri}, "ui/resourceUri": resource_uri}


def register_viewer(
    server: FastMCP,
    *,
    uri: str = VIEWER_URI,
    name: str = "ix-mcp tool result viewer",
    description: str = "Interactive HTML view of an ix-mcp tool result",
    html: str | None = None,
) -> dict[str, Any]:
    """Declare the viewer as a ``ui://`` resource on ``server``.

    Returns the ``meta=`` dict opting a tool in, so the whole wiring is::

        _UI_META = mcp_ui.register_viewer(mcp)

        @mcp.tool(structured_output=False, meta=_UI_META)
        async def my_tool(...) -> mcp_types.CallToolResult:
            ...
            return mcp_ui.ui_result(content, fragments=...)

    ``html`` swaps in a custom view document (a bespoke app for one server);
    the default is this module's generic result viewer.
    """
    document = viewer_html() if html is None else html

    def _viewer() -> str:
        return document

    server.resource(uri, name=name, description=description, mime_type=UI_MIME)(_viewer)
    return tool_meta(uri)


def html_fragments(cell_outputs: Sequence[Mapping[str, Any]]) -> list[str]:
    """The ``text/html`` fragments a run displayed, in display order.

    These are exactly what the dashboard and the room render for the human (a
    Result's ``user_html``, a styled DataFrame, a plot's HTML form), pulled from
    the run's nbformat-style outputs. Internal job-summary bundles are skipped;
    values arrive as one string or nbformat's list-of-lines.
    """
    from . import outputs as outputs_mod

    fragments: list[str] = []
    for output in cell_outputs:
        if output.get("output_type") not in ("execute_result", "display_data"):
            continue
        data = output.get("data")
        if not isinstance(data, Mapping) or outputs_mod.JOB_MIME in data:
            continue
        raw = data.get("text/html")
        if isinstance(raw, str) and raw.strip():
            fragments.append(raw)
        elif isinstance(raw, list):
            joined = "".join(part for part in raw if isinstance(part, str))
            if joined.strip():
                fragments.append(joined)
    return fragments


def _clip_fragments(fragments: Sequence[str], budget: int) -> list[str]:
    """Fit fragments to ``budget`` total chars, dropping from the tail.

    A partial fragment is worse than none (unbalanced markup), so fragments are
    kept whole. Truncation is loud: a visible notice fragment replaces what was
    dropped, pointing at the dashboard where the full view lives.
    """
    kept: list[str] = []
    used = 0
    for fragment in fragments:
        if used + len(fragment) > budget:
            dropped = len(fragments) - len(kept)
            kept.append(
                "<p class='ix-truncated'><em>"
                f"{dropped} view fragment(s) omitted to fit the reply "
                "(the full view is in the ix dashboard / room results panel)."
                "</em></p>"
            )
            break
        kept.append(fragment)
        used += len(fragment)
    return kept


def result_payload(
    content: Sequence[mcp_types.TextContent | mcp_types.ImageContent],
    *,
    fragments: Sequence[str] | None = None,
    title: str | None = None,
    budget: int = HTML_BUDGET,
) -> dict[str, Any]:
    """The CallToolResult-shaped dict the view renders, as plain JSON data.

    Shared by both transports: :func:`ui_result` sends it as the real tool
    result (the host relays it via ``ui/notifications/tool-result``);
    :func:`embedded_html` bakes it into the document for the room/data-API
    path. ``content`` is serialized with the wire aliases (``_meta`` etc.) so
    both sides see the same shape the spec names.
    """
    meta: dict[str, Any] = {}
    clipped = _clip_fragments(list(fragments or ()), budget)
    if clipped or title:
        view: dict[str, Any] = {}
        if title:
            view["title"] = title
        if clipped:
            view["html"] = clipped
        meta[RESULT_META_KEY] = view
    payload: dict[str, Any] = {
        "content": [block.model_dump(mode="json", by_alias=True, exclude_none=True) for block in content],
    }
    if meta:
        payload["_meta"] = meta
    return payload


def ui_result(
    content: Sequence[mcp_types.TextContent | mcp_types.ImageContent],
    *,
    fragments: Sequence[str] | None = None,
    title: str | None = None,
    budget: int = HTML_BUDGET,
) -> mcp_types.CallToolResult:
    """Wrap a tool's content blocks for an MCP Apps host.

    The blocks pass through UNCHANGED (the model sees exactly what it did
    before); the human view -- the run's HTML fragments -- rides in ``_meta``
    under :data:`RESULT_META_KEY`, which the host forwards to the view and
    every other consumer ignores. FastMCP passes a returned ``CallToolResult``
    through verbatim (mcp>=1.26, ``FuncMetadata.convert_result``), so ``_meta``
    survives to the wire.
    """
    meta = result_payload(content, fragments=fragments, title=title, budget=budget).get("_meta")
    return mcp_types.CallToolResult(content=list(content), _meta=meta)


def embedded_html(payload: Mapping[str, Any]) -> str:
    """The viewer document with ``payload`` (a :func:`result_payload` dict)
    baked in, for rendering OUTSIDE an MCP host -- the data API serves this at
    ``/api/jobs/{id}/ui`` and the room mounts it in a sandboxed
    (``allow-scripts``, opaque-origin) iframe. ``</`` is escaped so payload
    text can never close the carrier script tag.
    """
    encoded = json.dumps(payload).replace("<", "\\u003c")
    document = viewer_html()
    if _EMBED_MARKER not in document:
        raise ValueError("viewer template lost its embedded-payload marker")
    return document.replace(
        _EMBED_MARKER,
        f'<script type="application/json" id="ix-embedded-payload">{encoded}</script>',
    )


def job_payload(job: Mapping[str, Any]) -> dict[str, Any]:
    """A :func:`result_payload` for one stored execution row (``store.get``
    shape), so the room's per-job UI view shows the same thing an MCP Apps
    host would: the status header, the error if it failed, and the run's HTML
    fragments."""
    header = {
        "job": job.get("id"),
        "status": job.get("status"),
        "elapsed_s": (
            round(float(job["ended_at"]) - float(job["started_at"]), 3)
            if job.get("ended_at") is not None and job.get("started_at") is not None
            else None
        ),
    }
    content: list[mcp_types.TextContent | mcp_types.ImageContent] = [
        mcp_types.TextContent(type="text", text=json.dumps(header))
    ]
    if job.get("status") == "error" and job.get("error"):
        content.append(mcp_types.TextContent(type="text", text=str(job["error"])))
    outputs_field = job.get("outputs")
    cell_outputs: Sequence[Mapping[str, Any]] = outputs_field if isinstance(outputs_field, list) else []
    fragments = html_fragments(cell_outputs)
    if not fragments:
        # No rich view: fall back to the run's plain result/output so the frame
        # is never blank (blank would read as breakage; team norm: fail loudly).
        text = str(job.get("result") or job.get("output") or "(no output)")
        content.append(mcp_types.TextContent(type="text", text=text))
    return result_payload(
        content,
        fragments=fragments,
        title=str(job.get("name") or job.get("id") or "run"),
        # The room iframe has no 150k-chars-per-tool-result ceiling; give the
        # embedded view a roomier budget so a big table survives locally.
        budget=max(HTML_BUDGET, 400_000),
    )


def viewer_html() -> str:
    """The self-contained view document (no external origins, so the spec's
    restrictive default CSP applies as-is). Implements the MCP Apps view
    lifecycle by hand over ``postMessage`` JSON-RPC:

    - sends ``ui/initialize`` (protocolVersion ``2026-01-26``), applies the
      returned host context (theme + style variables), then signals
      ``ui/notifications/initialized``;
    - renders ``ui/notifications/tool-result`` (and surfaces
      ``ui/notifications/tool-cancelled``);
    - answers ``ping`` and ``ui/resource-teardown`` requests;
    - reports its size via ``ui/notifications/size-changed`` (ResizeObserver).

    Rendering: the ``_meta`` view fragments (the dashboard's HTML) when
    present, else the content blocks -- a JSON status header becomes a chip
    strip, long text collapses behind ``<details>``, images inline. Cheap
    interactivity: every rendered table gets click-to-sort headers.
    """
    return _VIEWER_HTML


_VIEWER_HTML = (
    """<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<meta name="color-scheme" content="light dark">
<style>
  :root { color-scheme: light dark; }
  html, body { margin: 0; padding: 0; background: transparent; }
  body {
    font-family: var(--font-sans, ui-sans-serif, system-ui, sans-serif);
    color: var(--color-text-primary, CanvasText);
    font-size: var(--font-text-md-size, 14px);
    line-height: 1.5;
    padding: 8px 10px;
  }
  #root { max-width: 100%; }
  .chips { display: flex; flex-wrap: wrap; gap: 6px; margin: 0 0 8px; }
  .chip {
    font-family: var(--font-mono, ui-monospace, monospace);
    font-size: 11px;
    padding: 2px 8px;
    border: 1px solid var(--color-border-primary, color-mix(in srgb, CanvasText 20%, transparent));
    border-radius: 999px;
    background: var(--color-background-secondary, color-mix(in srgb, CanvasText 5%, transparent));
  }
  .chip.status-error { border-color: #d33; color: #d33; }
  .chip.status-done, .chip.status-ok { border-color: #2a7; }
  .card { margin: 0 0 10px; }
  .frag { overflow-x: auto; }
  pre.text {
    font-family: var(--font-mono, ui-monospace, monospace);
    font-size: 12px;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    margin: 0 0 8px;
    padding: 8px 10px;
    border: 1px solid var(--color-border-primary, color-mix(in srgb, CanvasText 15%, transparent));
    border-radius: 8px;
    background: var(--color-background-secondary, color-mix(in srgb, CanvasText 4%, transparent));
    max-height: 420px;
    overflow-y: auto;
  }
  details { margin: 0 0 8px; }
  details > summary {
    cursor: pointer;
    font-size: 12px;
    color: var(--color-text-secondary, color-mix(in srgb, CanvasText 65%, transparent));
    user-select: none;
  }
  img.blob { max-width: 100%; border-radius: 8px; display: block; margin: 0 0 8px; }
  table { border-collapse: collapse; font-size: 12px; }
  table th, table td {
    padding: 3px 8px;
    border: 1px solid var(--color-border-primary, color-mix(in srgb, CanvasText 15%, transparent));
  }
  table th { cursor: pointer; user-select: none; background: var(--color-background-secondary, color-mix(in srgb, CanvasText 6%, transparent)); }
  table th.sorted-asc::after { content: " \\2191"; }
  table th.sorted-desc::after { content: " \\2193"; }
  .ix-truncated, .state { font-size: 12px; color: var(--color-text-secondary, color-mix(in srgb, CanvasText 60%, transparent)); }
  h1.title { font-size: 13px; font-weight: 600; margin: 0 0 6px; }
</style>
</head>
<body>
<div id="root"><p class="state">Waiting for tool result…</p></div>
"""
    + _EMBED_MARKER
    + """
<script>
(function () {
  "use strict";
  var root = document.getElementById("root");
  var VIEW_META_KEY = "io.indexable.ix/ui";

  // ---- minimal JSON-RPC over postMessage (spec: "you don't need an SDK") ----
  var nextId = 1;
  var pending = {};
  function post(msg) { window.parent.postMessage(msg, "*"); }
  function request(method, params) {
    var id = nextId++;
    return new Promise(function (resolve, reject) {
      pending[id] = { resolve: resolve, reject: reject };
      post({ jsonrpc: "2.0", id: id, method: method, params: params });
    });
  }
  function notify(method, params) { post({ jsonrpc: "2.0", method: method, params: params }); }

  window.addEventListener("message", function (event) {
    var data = event.data;
    if (!data || data.jsonrpc !== "2.0") return;
    // Response to one of our requests.
    if (data.id !== undefined && data.method === undefined) {
      var waiter = pending[data.id];
      if (!waiter) return;
      delete pending[data.id];
      if (data.error) waiter.reject(new Error(data.error.message || "host error"));
      else waiter.resolve(data.result);
      return;
    }
    // Host request: answer ping and teardown so the host never hangs on us.
    if (data.id !== undefined && (data.method === "ping" || data.method === "ui/resource-teardown")) {
      post({ jsonrpc: "2.0", id: data.id, result: {} });
      return;
    }
    if (data.method === "ui/notifications/tool-result") render(data.params || {});
    else if (data.method === "ui/notifications/tool-cancelled") {
      root.innerHTML = "";
      root.appendChild(el("p", "state", "Tool call cancelled: " + ((data.params || {}).reason || "no reason given")));
    } else if (data.method === "ui/notifications/tool-input") {
      var args = (data.params || {}).arguments || {};
      if (typeof args.intent === "string" && args.intent && !document.querySelector("h1.title")) {
        root.insertBefore(el("h1", "title", args.intent), root.firstChild);
      }
    } else if (data.method === "ui/notifications/host-context-changed") {
      applyHostContext(data.params || {});
    }
  });

  // ---- theming from host context ----
  function applyHostContext(ctx) {
    if (!ctx) return;
    if (ctx.theme) document.documentElement.style.colorScheme = ctx.theme;
    var vars = ctx.styles && ctx.styles.variables;
    if (vars) {
      for (var key in vars) {
        if (typeof vars[key] === "string") document.documentElement.style.setProperty(key, vars[key]);
      }
    }
  }

  // ---- rendering ----
  function el(tag, cls, text) {
    var node = document.createElement(tag);
    if (cls) node.className = cls;
    if (text !== undefined) node.textContent = text;
    return node;
  }

  function renderStatusChips(parent, obj) {
    var chips = el("div", "chips");
    for (var key in obj) {
      if (obj[key] === null || obj[key] === undefined) continue;
      var chip = el("span", "chip", key + ": " + obj[key]);
      if (key === "status") chip.className += " status-" + obj[key];
      chips.appendChild(chip);
    }
    parent.appendChild(chips);
  }

  function renderText(parent, text) {
    var pre = el("pre", "text", text);
    if (text.length > 2000 || text.split("\\n").length > 24) {
      var details = el("details");
      var lines = text.split("\\n").length;
      details.appendChild(el("summary", null, "text output (" + lines + " lines, " + text.length + " chars)"));
      details.appendChild(pre);
      parent.appendChild(details);
    } else {
      parent.appendChild(pre);
    }
  }

  function renderContentBlock(parent, block, index) {
    if (block.type === "image" && block.data) {
      var img = el("img", "blob");
      var mime = /^image\\/(png|jpeg|gif|webp)$/.test(block.mimeType) ? block.mimeType : "image/png";
      img.src = "data:" + mime + ";base64," + block.data;
      img.alt = "tool result image";
      parent.appendChild(img);
      return;
    }
    if (block.type !== "text" || typeof block.text !== "string" || !block.text) return;
    // The leading compact-JSON header (job/status/elapsed) becomes a chip strip.
    if (index === 0 && block.text[0] === "{") {
      try {
        var obj = JSON.parse(block.text);
        if (obj && typeof obj === "object" && !Array.isArray(obj)) { renderStatusChips(parent, obj); return; }
      } catch (err) { /* not the header; fall through to text */ }
    }
    renderText(parent, block.text);
  }

  // Click-to-sort on every rendered table header: numeric-aware, ascending
  // then descending. Cheap interactivity that needs no host capabilities.
  function makeTablesSortable(scope) {
    var tables = scope.querySelectorAll("table");
    for (var t = 0; t < tables.length; t++) enhanceTable(tables[t]);
  }
  function enhanceTable(table) {
    var headers = table.querySelectorAll("th");
    for (var i = 0; i < headers.length; i++) {
      (function (idx, th) {
        th.addEventListener("click", function () { sortTable(table, idx, th); });
      })(i, headers[i]);
    }
  }
  function sortTable(table, column, th) {
    var body = table.tBodies[0];
    if (!body) return;
    var asc = !th.classList.contains("sorted-asc");
    var heads = table.querySelectorAll("th");
    for (var i = 0; i < heads.length; i++) heads[i].classList.remove("sorted-asc", "sorted-desc");
    th.classList.add(asc ? "sorted-asc" : "sorted-desc");
    var rows = Array.prototype.slice.call(body.rows);
    rows.sort(function (a, b) {
      var av = (a.cells[column] || {}).textContent || "";
      var bv = (b.cells[column] || {}).textContent || "";
      var an = parseFloat(av), bn = parseFloat(bv);
      var cmp = (!isNaN(an) && !isNaN(bn)) ? an - bn : av.localeCompare(bv);
      return asc ? cmp : -cmp;
    });
    for (var r = 0; r < rows.length; r++) body.appendChild(rows[r]);
  }

  function render(result) {
    root.innerHTML = "";
    var view = (result._meta && result._meta[VIEW_META_KEY]) || {};
    if (view.title) root.appendChild(el("h1", "title", view.title));
    var content = Array.isArray(result.content) ? result.content : [];
    for (var i = 0; i < content.length; i++) renderContentBlock(root, content[i], i);
    // The rich human view: the same HTML the dashboard/room renders. Injected
    // via innerHTML inside this sandboxed, opaque-origin document; embedded
    // <script> in fragments does not execute (innerHTML semantics), which is
    // the conservative choice for kernel-produced markup.
    var fragments = Array.isArray(view.html) ? view.html : [];
    for (var f = 0; f < fragments.length; f++) {
      var card = el("div", "card");
      var frag = el("div", "frag");
      frag.innerHTML = fragments[f];
      card.appendChild(frag);
      root.appendChild(card);
    }
    if (!content.length && !fragments.length) {
      root.appendChild(el("p", "state", "(empty tool result)"));
    }
    makeTablesSortable(root);
  }

  // ---- size reporting ----
  if (typeof ResizeObserver === "function") {
    var observer = new ResizeObserver(function () {
      notify("ui/notifications/size-changed", {
        width: document.documentElement.scrollWidth,
        height: document.documentElement.scrollHeight
      });
    });
    observer.observe(document.body);
  }

  // ---- boot: embedded payload (room / data API) or MCP Apps handshake ----
  var embedded = null;
  var carrier = document.getElementById("ix-embedded-payload");
  if (carrier) {
    try { embedded = JSON.parse(carrier.textContent); } catch (err) { embedded = null; }
  }
  if (embedded && typeof embedded === "object") {
    render(embedded);
    return;
  }
  request("ui/initialize", {
    protocolVersion: "2026-01-26",
    clientInfo: { name: "ix-mcp tool result viewer", version: "1.0.0" },
    capabilities: {},
    appCapabilities: { availableDisplayModes: ["inline"] }
  }).then(function (result) {
    applyHostContext(result && result.hostContext);
    notify("ui/notifications/initialized", {});
  }).catch(function (err) {
    root.innerHTML = "";
    root.appendChild(el("p", "state", "MCP Apps handshake failed: " + err.message));
  });
})();
</script>
</body>
</html>
"""
)
