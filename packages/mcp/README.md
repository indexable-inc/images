# ix-mcp

<!--
  DEMO (front door): the thing that sells this is motion — an agent writing and
  running cells while a human watches them appear in JupyterLab. Record a ~20s
  loop and drop in two theme variants, then replace the <p align="center"> hero
  image below with the <picture> block:

  <p align="center">
    <picture>
      <source media="(prefers-color-scheme: dark)"  srcset="docs/demo-dark.avif"  type="image/avif">
      <source media="(prefers-color-scheme: light)" srcset="docs/demo-light.avif" type="image/avif">
      <source media="(prefers-color-scheme: dark)"  srcset="docs/demo-dark.webp">
      <source media="(prefers-color-scheme: light)" srcset="docs/demo-light.webp">
      <img alt="An agent and a human co-editing one live Jupyter notebook" src="docs/demo-dark.webp" width="900">
    </picture>
  </p>

  Commit the files at packages/mcp/docs/demo-{dark,light}.{webp,avif}. GitHub
  strips author-written <video> tags, so an animated WebP/AVIF inside <picture>
  is the only thing that autoplays, loops, and swaps on dark/light. Encode the
  loop with ffmpeg -loop 0; see skills/github-readme-media for the recipe and
  size limits.
-->

<p align="center">
  <img width="720" alt="An agent renders a live htop screen into the shared notebook with the bundled tui driver" src="https://github.com/user-attachments/assets/e0815b1a-7126-4ba0-9315-a7db53615266" />
  <br>
  <sub><i>An agent renders a live htop screen into the notebook with the bundled <code>tui</code> driver. A human watching the same notebook in JupyterLab sees the cells and outputs appear as they happen.</i></sub>
</p>

A notebook-first MCP server. An AI agent and a human drive **one live Jupyter
notebook** together: every tool call edits a real `.ipynb` through Jupyter's
real-time-collaboration layer, so a person who opens the same notebook in
JupyterLab sees the agent's cells and outputs appear as they happen, and the work
is left behind as a notebook with outputs that anyone can reopen.

## Quickstart

```
nix run .#mcp -- serve            # MCP over stdio (what an MCP client launches)
nix run .#mcp -- serve --http :8000   # MCP over streamable HTTP instead
nix run .#mcp -- lab              # open the running server's JupyterLab URL
nix run .#mcp -- eval '1 + 2'     # one-shot expression on a throwaway kernel
```

When `serve` starts it prints a JupyterLab URL to stderr; open it, or run
`ix-mcp lab`, to co-edit the notebook the agent is working in. Jupyter auth is
disabled (no token, no password), so the URL opens straight in. Access is gated
by reachability instead: the default bind is loopback, and the fleet only
exposes the server over Tailscale (see [Remote access](#remote-access)). Never
bind it to a public interface, since a reachable Jupyter Server is arbitrary
code execution for whoever can dial it.

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
Google API client, and on macOS `screen` and `vmkit`. A per-notebook kernel is
shared with the human, so state set by an agent cell is visible in the browser.

<p align="center">
  <img width="760" alt="An agent drives lldb through the bundled tui module to a breakpoint in a C program, inside a notebook cell" src="https://github.com/user-attachments/assets/f7d34718-d44a-441a-9927-d367a725de04" />
  <br>
  <sub><i>The bundled <code>tui</code> driver lets a cell spawn and steer a real program: here an agent drives <code>lldb</code> to a breakpoint, rendered back into the notebook.</i></sub>
</p>

DataFrames render as interactive tables out of the box: every kernel loads
[itables](https://mwouts.github.io/itables/) at startup with
`init_notebook_mode(all_interactive=True)`, so displaying a pandas or polars
frame gives the human a sortable, searchable, paginated DataTable in JupyterLab
instead of a static table. The `text/plain` repr is kept alongside it, so the
agent (which reads text, not HTML) and any non-JS viewer are unchanged. The
DataTables library loads from a CDN (itables `connected=True`), the mode that
works from a startup script; a frame larger than 256KB is downsampled to a slice
with a banner so a single cell cannot embed megabytes into the notebook.

The agent reads that `text/plain` repr, so the kernel also widens polars'
defaults (up to 40 rows and columns, 80-char strings) so a frame is not
truncated to ~8 columns in the agent's view. The MCP layer still caps a single
text output at 50k chars, so the wider repr cannot flood the agent's context.

## Remote access

Two env vars control how a remote VM hands back a reachable URL:

- `IX_MCP_HOST`: the address Jupyter binds. The default is this node's
  Tailscale IPv4 (`100.x.y.z`) when Tailscale is up, so a phone or laptop on
  the same tailnet can open the lab URL without ssh. Falls back to `127.0.0.1`
  when Tailscale is not up. Set it to `0.0.0.0` to listen on every interface
  (do this only behind a host firewall), or to a specific address to override
  the auto-detected Tailscale IP.
- `IX_MCP_PUBLIC_HOST`: the host put into the lab URL. Set it to force a specific
  name (e.g. `myvm.tail368802.ts.net`).

The default Tailscale-IP bind keeps the trust boundary at the tailnet: only
tailnet peers can dial `100.x.y.z`, so the local Wi-Fi cannot reach the server.
If you bind a wildcard (`0.0.0.0`/`::`) without setting `IX_MCP_PUBLIC_HOST`, the
URL host is auto-resolved to a reachable name: the Tailscale MagicDNS name first,
then the FQDN, then `127.0.0.1` as a fallback.

## Bad fit if

- You need a fully offline, server-less notebook: `serve` always runs a Jupyter
  Server (that is what makes co-editing and the kernel work). For a quick
  scratch evaluation with no server, use `ix-mcp eval`/`exec`.
- You have many simultaneous human collaborators: Jupyter's RTC is solid for a
  small number of editors but has had sync rough edges reported at large scale.
