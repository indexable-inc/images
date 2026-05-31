---
name: ix-mcp
disclosure: progressive
description: "The ix MCP server (a pinned Python interpreter with bundled search, PTY, browser, and screen modules) is wired into Claude Code by default. Use when you want to run Python with top-level await, search the codebase semantically, drive a PTY, or automate a browser from inside a session."
---

## ix MCP server

Every Claude Code session in this repo gets the `ix` MCP server by default. It is
declared in [`.mcp.json`](.mcp.json) at the repo root and pre-approved through
`enabledMcpjsonServers` in [`.claude/settings.json`](.claude/settings.json), so
the `mcp__ix__*` tools are connected with no per-session approval prompt. The
server itself is the [`packages/mcp`](packages/mcp) crate (`ix-mcp`), launched
with `nix run .#mcp -- serve`; that flakeref is the single source of truth, so
the config tracks the current checkout with no separate build step.

The server is a single pinned Python interpreter, not a grab bag of shell tools.
Sessions are persistent and single-threaded with a live asyncio loop, so
top-level `await` works (never call `asyncio.run()`), and any async resource
created in one call stays alive for later calls.

Tools it exposes:

- `python_eval` / `python_exec` — evaluate an expression or run statements, with
  top-level await. Open figures and objects with a `_repr_png_` come back as MCP
  image blocks.
- `python_session_create` / `python_session_list` / `python_session_close` /
  `python_reset` — manage persistent named sessions.
- `search_semantic` / `search_grep` — semantic code search and regex grep over
  the indexed chunks, the same engine the `searching` skill describes.

Bundled in the interpreter so every session imports them with no install step:
`tui` (PTY driver for terminal automation), `search`, `playwright` (browsers
pre-cached, async API), `numpy`, `polars`, `matplotlib`, `asyncssh`, and on
macOS `screen` (screenshot, cursor, synthetic clicks). Each session also has a
writable venv, so an in-session `pip install` resolves to that venv.

Reach for it instead of shelling out when you want a persistent Python REPL,
semantic search results in-context, or browser/terminal automation that keeps
state across calls.
