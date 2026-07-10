"""Call any Model Context Protocol (MCP) server's tools from the kernel.

Bundled like ``view``/``search``/``sh`` so every session can ``import mcp_client``
with no setup. The point: an MCP server is a bag of tools (and resources and
prompts) behind a small JSON-RPC protocol -- a Todoist server, a GitHub server,
a local stdio server you wrote -- and this module lets you connect to one and
call those tools straight from Python, with results that render two ways like
every other kernel value.

It is a thin, ergonomic wrapper over the official ``mcp`` Python SDK (already
bundled): the SDK gives you raw ``async with`` transport + session context
managers that are awkward to keep open across notebook cells; this module hands
you a persistent :class:`Server` you connect once and reuse, driven by a single
background task so the SDK's anyio cancel scopes never cross tasks.

    import mcp_client

    # A local stdio server (a command). Args after the program are its argv.
    srv = await mcp_client.connect("python my_server.py")

    # A remote streamable-HTTP server, with a bearer token or custom headers.
    srv = await mcp_client.connect("https://host/mcp", token="…")

    srv                         # last expr: dashboard shows the tool table,
                                # you get the server name + tools as text
    srv.tools                   # a polars DataFrame: name, summary, params
    out = await srv.call("create_task", content="ship it", due="today")
    out                         # a ToolResult: text for you, rich HTML for the
                                # human, any image blocks as real images
    out.data                    # the tool's structuredContent (parsed JSON)
    await srv.close()           # or `async with await connect(...) as srv:`

``connect`` picks the transport from the target: an ``http(s)://`` URL uses the
streamable-HTTP transport (pass ``transport="sse"`` for an older SSE server),
anything else is treated as a stdio command (a string is split with ``shlex``;
pass a list to skip splitting). Authentication: pass a static bearer ``token=``
or arbitrary ``headers=`` and they are used as-is. Otherwise a remote server
that answers 401 triggers the interactive OAuth 2.0 + PKCE flow automatically:
the browser opens for consent (headless sessions get the URL printed instead),
a loopback listener on ``127.0.0.1`` catches the redirect, and the tokens are
cached under ``$XDG_STATE_HOME/ix-mcp/oauth`` (default
``~/.local/state/ix-mcp/oauth``, files ``0600``) so the next ``connect`` needs
no browser and expiring tokens refresh transparently. Servers without dynamic
client registration take ``client_id=`` (and ``scopes=``); pass ``oauth=False``
to disable the flow entirely.

Open servers are tracked in :data:`mcp_client.servers` (like ``jobs``), so you
can list and close them: ``mcp_client.servers`` is the live dict, and
``await mcp_client.close_all()`` shuts every one down.
"""

from __future__ import annotations

import asyncio
import json as _json
import shlex
from typing import Any

import polars as pl

from mcp import ClientSession, StdioServerParameters, types
from mcp.client.sse import sse_client
from mcp.client.stdio import stdio_client
from mcp.client.streamable_http import streamablehttp_client

__all__ = ["MCPError", "Server", "ToolResult", "close_all", "connect", "servers"]

__version__ = "0.1.0"

# `Result` is the kernel runtime's human/model split: importing it lets a
# `ToolResult` BE a Result, so a cell can end with `await srv.call(...)` and
# satisfy the contract with no `Result.of(...)` wrapper. Outside the kernel
# (plain `import mcp_client` in a script or test) the runtime is absent; fall
# back to `object` so the module still imports and the reprs carry rendering.
try:
    from ix_notebook_mcp.runtime import Result as _ResultBase

    _HAS_RESULT = True
except Exception:  # pragma: no cover - exercised only outside the kernel
    _ResultBase = object
    _HAS_RESULT = False


def _esc(text: str) -> str:
    return str(text).replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


class MCPError(RuntimeError):
    """Raised when an MCP connection or tool call fails."""


# --- the live registry of open servers (like `jobs`) -------------------------

servers: dict[str, Server] = {}


async def close_all() -> None:
    """Close every open server. Safe to call repeatedly."""
    for srv in list(servers.values()):
        await srv.close()


# --- tool results ------------------------------------------------------------


class ToolResult(_ResultBase):
    """The outcome of a tool call, split into a human view and a model view.

    Ending a cell with one renders the tool's text (and any image blocks) for the
    model and a styled panel for the human. The parts are also available
    programmatically:

        out.text     # the tool's text content, joined
        out.data     # structuredContent (parsed JSON), or None
        out.content  # the raw list of MCP content blocks
        out.is_error # True if the tool reported a failure
    """

    def __init__(self, tool: str, result: types.CallToolResult) -> None:
        self.tool = tool
        self.content = list(result.content or [])
        self.data = result.structuredContent
        self.is_error = bool(result.isError)

        texts: list[str] = []
        images: list[bytes] = []
        for block in self.content:
            btype = getattr(block, "type", None)
            if btype == "text":
                texts.append(block.text)
            elif btype == "image":
                try:
                    import base64

                    images.append(base64.b64decode(block.data))
                except Exception:  # noqa: S110 -- malformed image block: skip silently, text still delivered
                    pass
        self.text = "\n".join(texts)

        # Model view: prefer the tool's text, else its structured data as JSON.
        body = self.text
        if not body and self.data is not None:
            body = _json.dumps(self.data, indent=2, default=str)
        if self.is_error:
            body = f"[tool error] {body}" if body else "[tool error]"
        self.llm_result = body
        self.llm_images = images

        # Human view: a titled panel with the text and, when present, the JSON.
        rows = [
            f'<div class="ix-mcp-tool">{_esc(tool)}{" ⚠" if self.is_error else ""}</div>'
        ]
        if self.text:
            rows.append(f'<pre class="ix-result">{_esc(self.text)}</pre>')
        if self.data is not None:
            rows.append(
                f'<pre class="ix-result">{_esc(_json.dumps(self.data, indent=2, default=str))}</pre>'
            )
        if not self.text and self.data is None and not images:
            rows.append('<pre class="ix-result">(no content)</pre>')
        self.user_html = "\n".join(rows)

    def __repr__(self) -> str:
        flag = " error" if self.is_error else ""
        return f"<ToolResult {self.tool}{flag}: {self.text[:200]!r}>"

    def _repr_html_(self) -> str:
        return self.user_html


# --- the persistent server connection ----------------------------------------

_STOP = object()


class Server:
    """A live connection to one MCP server.

    Construct via :func:`connect` (not directly). All requests are funnelled
    through a single background task that owns the SDK session, so the transport's
    anyio cancel scopes are entered and exited in the same task no matter which
    cell calls a method.
    """

    def __init__(self, key: str, transport_factory: object, label: str) -> None:
        self._key = key
        self._transport_factory = transport_factory
        self.label = label
        self._queue: asyncio.Queue = asyncio.Queue()
        self._task: asyncio.Task | None = None
        self._ready = asyncio.Event()
        self._closed = asyncio.Event()
        self._error: BaseException | None = None
        self.info: dict[str, Any] = {}
        self.tools: pl.DataFrame = pl.DataFrame()
        self.resources: pl.DataFrame = pl.DataFrame()
        self.prompts: pl.DataFrame = pl.DataFrame()
        self._tools_raw: list = []

    # -- lifecycle --

    async def _open(self) -> Server:
        self._task = asyncio.create_task(self._run(), name=f"mcp:{self.label}")
        ready = asyncio.create_task(self._ready.wait())
        _done, _ = await asyncio.wait(
            {ready, self._task}, return_when=asyncio.FIRST_COMPLETED
        )
        if not self._ready.is_set():
            # The runner finished before signalling ready -> it failed to start.
            ready.cancel()
            raise self._error or MCPError(
                f"{self.label}: connection closed before init"
            )
        ready.cancel()
        if self._error is not None:
            raise self._error
        servers[self._key] = self
        return self

    async def _run(self) -> None:
        try:
            async with self._transport_factory() as streams:
                read, write = streams[0], streams[1]
                async with ClientSession(read, write) as session:
                    init = await session.initialize()
                    self.info = {
                        "name": init.serverInfo.name,
                        "version": init.serverInfo.version,
                        "protocol": init.protocolVersion,
                        "instructions": init.instructions,
                    }
                    await self._load_catalog(session)
                    self._ready.set()
                    while True:
                        item = await self._queue.get()
                        if item is _STOP:
                            break
                        fn, fut = item
                        try:
                            res = await fn(session)
                            if not fut.done():
                                fut.set_result(res)
                        except Exception as exc:
                            if not fut.done():
                                fut.set_exception(exc)
        except Exception as exc:
            self._error = exc
            self._ready.set()
        finally:
            self._closed.set()
            servers.pop(self._key, None)
            # Fail anything still queued so no caller awaits forever.
            while not self._queue.empty():
                item = self._queue.get_nowait()
                if item is _STOP:
                    continue
                _fn, fut = item
                if not fut.done():
                    fut.set_exception(self._error or MCPError(f"{self.label}: closed"))

    async def _submit(self, fn: object) -> object:
        if self._closed.is_set():
            raise MCPError(f"{self.label}: server is closed")
        fut: asyncio.Future = asyncio.get_event_loop().create_future()
        await self._queue.put((fn, fut))
        return await fut

    async def close(self) -> None:
        """Shut the connection down and drop it from :data:`servers`."""
        if self._task is None or self._closed.is_set():
            servers.pop(self._key, None)
            return
        await self._queue.put(_STOP)
        try:
            await asyncio.wait_for(self._closed.wait(), timeout=10)
        except TimeoutError:
            self._task.cancel()

    async def __aenter__(self) -> Server:
        return self

    async def __aexit__(self, *exc: object) -> None:
        await self.close()

    # -- catalog --

    async def _load_catalog(self, session: ClientSession) -> None:
        try:
            tools = (await session.list_tools()).tools
        except Exception:
            tools = []
        self._tools_raw = tools
        self.tools = _tools_frame(tools)
        try:
            res = (await session.list_resources()).resources
        except Exception:
            res = []
        self.resources = _resources_frame(res)
        try:
            prompts = (await session.list_prompts()).prompts
        except Exception:
            prompts = []
        self.prompts = _prompts_frame(prompts)

    async def refresh(self) -> Server:
        """Re-fetch the tool / resource / prompt catalog from the server."""
        await self._submit(self._load_catalog)
        return self

    # -- calls --

    async def call(
        self, tool: str, arguments: dict | None = None, /, **kwargs: object
    ) -> ToolResult:
        """Call ``tool`` with keyword arguments (or a single ``arguments`` dict).

        Returns a :class:`ToolResult` (a Result), so a cell can end with the call.
        Raises :class:`MCPError` if the tool is unknown to this server.
        """
        args = dict(arguments or {})
        args.update(kwargs)
        known = set(self.tools["name"].to_list()) if self.tools.height else set()
        if known and tool not in known:
            raise MCPError(
                f"{self.label}: no tool {tool!r}; available: {', '.join(sorted(known)) or '(none)'}"
            )

        async def _do(session: ClientSession) -> object:
            return await session.call_tool(tool, args)

        result = await self._submit(_do)
        return ToolResult(tool, result)

    async def read(self, uri: str) -> object:
        """Read a resource by URI; returns the SDK ``ReadResourceResult``."""

        async def _do(session: ClientSession) -> object:
            return await session.read_resource(uri)

        return await self._submit(_do)

    async def prompt(self, name: str, arguments: dict | None = None, /, **kwargs: object) -> object:
        """Fetch a prompt by name; returns the SDK ``GetPromptResult``."""
        args = dict(arguments or {})
        args.update(kwargs)

        async def _do(session: ClientSession) -> object:
            return await session.get_prompt(name, args)

        return await self._submit(_do)

    # -- rendering --

    def __repr__(self) -> str:
        name = self.info.get("name", self.label)
        return f"<mcp_client.Server {name!r}: {self.tools.height} tools>"

    def _repr_html_(self) -> str:
        name = self.info.get("name", self.label)
        ver = self.info.get("version", "")
        head = f'<div class="ix-mcp-server"><b>{_esc(name)}</b> {_esc(ver)} — {self.tools.height} tools</div>'
        try:
            body = self.tools._repr_html_() if self.tools.height else "<i>no tools</i>"
        except Exception:
            body = "<i>no tools</i>"
        return head + body

    # Result contract: a bare `srv` at the end of a cell renders the tool table
    # for the human and a compact text summary for the model.
    @property
    def user_html(self) -> str:
        return self._repr_html_()

    @property
    def llm_result(self) -> str:
        name = self.info.get("name", self.label)
        lines = [
            f"{name} ({self.info.get('version', '?')}) — {self.tools.height} tools"
        ]
        if self.tools.height:
            lines.extend(
                f"  {row['name']}({row.get('params', '')}) — {row.get('summary', '')}"
                for row in self.tools.iter_rows(named=True)
            )
        return "\n".join(lines)

    @property
    def llm_images(self) -> list:
        return []


# --- frame builders ----------------------------------------------------------


def _summary(text: str | None) -> str:
    return (text or "").strip().split("\n", 1)[0]


def _params(schema: dict | None) -> str:
    if not schema:
        return ""
    props = schema.get("properties") or {}
    required = set(schema.get("required") or [])
    parts = []
    for key, spec in props.items():
        typ = spec.get("type", "any") if isinstance(spec, dict) else "any"
        parts.append(f"{key}: {typ}" + ("" if key in required else "?"))
    return ", ".join(parts)


def _tools_frame(tools: list) -> pl.DataFrame:
    rows = [
        {
            "name": t.name,
            "summary": _summary(getattr(t, "description", "")),
            "params": _params(getattr(t, "inputSchema", None)),
        }
        for t in tools
    ]
    return pl.DataFrame(
        rows, schema={"name": pl.Utf8, "summary": pl.Utf8, "params": pl.Utf8}
    )


def _resources_frame(resources: list) -> pl.DataFrame:
    rows = [
        {
            "uri": str(getattr(r, "uri", "")),
            "name": getattr(r, "name", "") or "",
            "summary": _summary(getattr(r, "description", "")),
            "mime": getattr(r, "mimeType", "") or "",
        }
        for r in resources
    ]
    return pl.DataFrame(
        rows,
        schema={"uri": pl.Utf8, "name": pl.Utf8, "summary": pl.Utf8, "mime": pl.Utf8},
    )


def _prompts_frame(prompts: list) -> pl.DataFrame:
    rows = [
        {
            "name": p.name,
            "summary": _summary(getattr(p, "description", "")),
            "args": ", ".join(a.name for a in (getattr(p, "arguments", None) or [])),
        }
        for p in prompts
    ]
    return pl.DataFrame(
        rows, schema={"name": pl.Utf8, "summary": pl.Utf8, "args": pl.Utf8}
    )


# --- connect -----------------------------------------------------------------


async def connect(
    target: str | list[str],
    *,
    token: str | None = None,
    headers: dict[str, str] | None = None,
    env: dict[str, str] | None = None,
    cwd: str | None = None,
    transport: str = "auto",
    timeout: float = 30,
    name: str | None = None,
    oauth: bool | str = "auto",
    client_id: str | None = None,
    client_secret: str | None = None,
    scopes: str | list[str] | None = None,
    oauth_timeout: float = 300,
) -> Server:
    """Connect to an MCP server and return a ready :class:`Server`.

    ``target`` is either an ``http(s)://`` URL (a remote server) or a stdio
    command (a string, split with ``shlex``, or a pre-split list). ``transport``
    is inferred from the target (``"http"`` for a URL, ``"stdio"`` otherwise);
    pass ``"sse"`` for an older Server-Sent-Events HTTP server. Authentication is
    a bearer ``token=`` or arbitrary ``headers=``; ``env`` / ``cwd`` apply to a
    stdio child process. The catalog (tools / resources / prompts) is fetched
    before returning, so ``srv.tools`` is populated immediately.

    Remote servers that answer 401 get the interactive OAuth 2.0 + PKCE flow
    by default (``oauth="auto"``: enabled whenever no ``token=`` and no
    ``Authorization`` header is given). The browser opens for consent -- or, in
    a headless session, the URL is printed -- and tokens are cached per server
    so the next ``connect`` is silent; ``oauth_timeout`` bounds the wait for
    the human. ``client_id=`` / ``client_secret=`` pin a pre-registered OAuth
    client for servers without dynamic registration, ``scopes=`` requests
    specific scopes, and ``oauth=False`` disables the flow entirely.
    """
    is_url = isinstance(target, str) and target.startswith(("http://", "https://"))
    if transport == "auto":
        transport = "http" if is_url else "stdio"

    hdrs = dict(headers or {})
    if token:
        hdrs.setdefault("Authorization", f"Bearer {token}")

    open_timeout = timeout + 5
    if transport in ("http", "sse"):
        if not isinstance(target, str):
            raise MCPError("an http/sse target must be a URL string")
        url = target
        if transport == "http":

            def base_factory(auth: object = None, url: str = url, hdrs: dict = hdrs, timeout: float = timeout) -> object:
                return streamablehttp_client(
                    url, headers=hdrs or None, timeout=timeout, auth=auth
                )
        else:

            def base_factory(auth: object = None, url: str = url, hdrs: dict = hdrs) -> object:
                return sse_client(url, headers=hdrs or None, auth=auth)

        has_auth_header = any(k.lower() == "authorization" for k in hdrs)
        use_oauth = oauth is True or (oauth == "auto" and not has_auth_header)
        if use_oauth:
            from . import _oauth

            def factory(url: str = url) -> object:
                return _oauth.oauth_transport(
                    base_factory,
                    url,
                    client_id=client_id,
                    client_secret=client_secret,
                    scopes=scopes,
                    flow_timeout=oauth_timeout,
                )

            # The consent wait is human-paced: only the OAuth flow's own
            # timeout should bound it, not the transport connect timeout.
            open_timeout += oauth_timeout
        else:
            factory = base_factory

        label = name or url
        key = f"{transport}:{url}"
    elif transport == "stdio":
        argv = shlex.split(target) if isinstance(target, str) else list(target)
        if not argv:
            raise MCPError("empty stdio command")
        params = StdioServerParameters(command=argv[0], args=argv[1:], env=env, cwd=cwd)

        def factory(params: StdioServerParameters = params) -> object:
            return stdio_client(params)

        label = name or " ".join(argv)
        key = f"stdio:{label}"
    else:
        raise MCPError(f"unknown transport {transport!r}; use auto/http/sse/stdio")

    srv = Server(key, factory, label)
    try:
        await asyncio.wait_for(srv._open(), timeout=open_timeout)
    except TimeoutError as exc:
        await srv.close()
        raise MCPError(f"{label}: timed out connecting") from exc
    return srv
