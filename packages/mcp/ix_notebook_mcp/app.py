"""The notebook application: the typed owner of the running server.

`NotebookApp` wraps the live Jupyter `serverapp` and is the one place that knows
how to reach a notebook's collaborative document and its kernel. The MCP tools
hold no server state of their own; they go through the single `NotebookApp`
instance the extension builds at startup (`current_app()`).
"""

from __future__ import annotations

import nbformat
from jupyter_ydoc import YNotebook

from .config import Config
from .outputs import output_from_message

# Cap on the kernel readiness handshake, independent of the cell's own timeout.
_READY_TIMEOUT = 30.0


class NotebookApp:
    def __init__(self, config: Config, serverapp: object) -> None:
        self._config = config
        self._serverapp = serverapp

    @property
    def config(self) -> Config:
        return self._config

    async def live_notebook(self, rel_path: str) -> YNotebook:
        """Return the live collaborative ``YNotebook`` for ``rel_path``.

        Uses ``jupyter_server_ydoc``'s public ``get_document`` with ``copy=False``
        so this is the *actual* room document (not a snapshot): edits propagate to
        every connected browser and are persisted to the ``.ipynb`` by the
        collaboration layer. ``create=True`` opens the room server-side even
        before any browser connects, so the agent can build a notebook and a human
        can then join it. Editing this object (rather than writing the file) is
        what keeps a co-editing browser from desyncing.
        """
        ydoc_ext = self._serverapp.extension_manager.extension_points["jupyter_server_ydoc"].app
        document = await ydoc_ext.get_document(
            path=rel_path,
            content_type="notebook",
            file_format="json",
            copy=False,
            create=True,
        )
        if document is None:
            raise RuntimeError(f"could not open collaborative document for {rel_path!r}")
        return document

    def ensure_file(self, rel_path: str) -> str:
        """Create an empty valid notebook on disk if missing, and return its
        canonical workspace-relative path.

        A file must exist before the file-id manager can key its YDoc room, so
        notebook creation is "write an empty .ipynb, then open its room".
        """
        if not rel_path.endswith(".ipynb"):
            rel_path = f"{rel_path}.ipynb"
        abspath = self._config.resolve(rel_path)
        if not abspath.exists():
            abspath.parent.mkdir(parents=True, exist_ok=True)
            nbformat.write(nbformat.v4.new_notebook(), abspath)
        return self._config.canonical(rel_path)

    async def kernel_id(self, rel_path: str) -> str:
        """Return the kernel id for ``rel_path``'s session, creating the session
        (and kernel) if needed. Keying the session on the path means the browser
        and the agent converge on one shared kernel for a given notebook."""
        sessions = self._serverapp.session_manager
        if await sessions.session_exists(path=rel_path):
            model = await sessions.get_session(path=rel_path)
            return model["kernel"]["id"]
        model = await sessions.create_session(
            path=rel_path, name=rel_path, type="notebook", kernel_name="python3"
        )
        return model["kernel"]["id"]

    async def restart_kernel(self, rel_path: str) -> None:
        kernel_id = await self.kernel_id(rel_path)
        await self._serverapp.kernel_manager.restart_kernel(kernel_id)

    async def execute(self, rel_path: str, code: str, timeout: float) -> tuple[list[dict], int | None]:
        """Run ``code`` on the notebook's kernel; return (nbformat outputs, count).

        Connects as an extra client on the kernel (its IOPub is a PUB socket, so
        every client gets its own copy of the messages; the agent never steals the
        browser's output) and drives the execution with ``execute_interactive``,
        which owns the shell/iopub coordination and idle detection. Raises
        ``TimeoutError`` if the kernel does not finish within ``timeout``; the
        kernel stays alive for the next call.
        """
        kernel = self._serverapp.kernel_manager.get_kernel(await self.kernel_id(rel_path))
        client = kernel.client()
        client.start_channels()
        try:
            # The kernel is already started (the session created it), so readiness
            # is a quick handshake; cap it well under `timeout` so a wedged kernel
            # cannot cost up to 2x the budget (ready wait + execute wait).
            await client.wait_for_ready(timeout=min(timeout, _READY_TIMEOUT))
            outputs: list[dict] = []
            execution_count: int | None = None

            def on_iopub(msg: dict) -> None:
                nonlocal execution_count
                if msg["msg_type"] == "execute_input":
                    execution_count = msg["content"].get("execution_count", execution_count)
                output = output_from_message(msg)
                if output is not None:
                    outputs.append(output)

            reply = await client.execute_interactive(
                code,
                timeout=timeout,
                allow_stdin=False,
                output_hook=on_iopub,
                store_history=True,
            )
            execution_count = reply["content"].get("execution_count", execution_count)
            return outputs, execution_count
        finally:
            client.stop_channels()

    def lab_url(self) -> str:
        return self._config.lab_url()


_APP: NotebookApp | None = None


def set_app(app: NotebookApp) -> None:
    global _APP
    _APP = app


def current_app() -> NotebookApp:
    if _APP is None:
        raise RuntimeError("the notebook app is not running; call a tool inside `ix-mcp serve`")
    return _APP
