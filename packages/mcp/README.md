# ix-mcp

A notebook-first MCP server. An AI agent and a human drive **one live Jupyter
notebook** together: every tool call edits a real `.ipynb` through Jupyter's
real-time-collaboration layer, so a person who opens the same notebook in
JupyterLab sees the agent's cells and outputs appear as they happen, and the work
is left behind as a notebook with outputs that anyone can reopen.

```
nix run .#mcp -- serve            # MCP over stdio (what an MCP client launches)
nix run .#mcp -- serve --http :8000   # MCP over streamable HTTP instead
nix run .#mcp -- lab              # open the running server's JupyterLab URL
nix run .#mcp -- eval '1 + 2'     # one-shot expression on a throwaway kernel
```

When `serve` starts it prints a JupyterLab URL (with an auth token) to stderr;
open it, or run `ix-mcp lab`, to co-edit the notebook the agent is working in.

## How it works

`ix-mcp serve` launches a Jupyter Server in the same process as the MCP server,
with `jupyter_server_ydoc` (the server side of real-time collaboration) enabled.
Running in one process is the point: the MCP tools edit the exact `YNotebook`
(a CRDT) that a browser is subscribed to, so edits broadcast live and persist to
disk instead of fighting the browser with out-of-band file writes.

- `runtime.py` holds the process-global config the CLI hands the extension.
- `notebook.py` reaches the live `YNotebook` for a path and edits its cells.
- `kernel.py` runs code on the notebook's own kernel and turns kernel messages
  into notebook outputs (text, results, images).
- `tools.py` is the MCP tool surface (`notebook_use`, `cell_add`, `cell_run`,
  `cell_overwrite`, `cell_delete`, `run_code`, `kernel_restart`,
  `notebook_read`, `notebook_list`, plus `search_semantic`/`search_grep` over the
  shared `index` corpus).
- `extension.py` is the Jupyter Server extension that binds the running server
  into the runtime and starts the MCP transport on its event loop.
- `serve.py` serves the tools over stdio (handing the protocol the real stdout so
  the Jupyter Server's logging cannot corrupt the JSON-RPC stream) or HTTP.

## Pinned interpreter and bundled modules

The kernel runs on the same pinned interpreter as the server (see
[`default.nix`](./default.nix)), so notebook cells can `import` the bundled
modules with no install step: `tui` (PTY driver), `search` (semantic/grep over
the `index` corpus), numpy, polars, duckdb, httpx, matplotlib, playwright, the
Google API client, and on macOS `screen` and `macvm`. A per-notebook kernel is
shared with the human, so state set by an agent cell is visible in the browser.

## Bad fit if

- You need a fully offline, server-less notebook: `serve` always runs a Jupyter
  Server (that is what makes co-editing and the kernel work). For a quick
  scratch evaluation with no server, use `ix-mcp eval`/`exec`.
- You have many simultaneous human collaborators: Jupyter's RTC is solid for a
  small number of editors but has had sync rough edges reported at large scale.
