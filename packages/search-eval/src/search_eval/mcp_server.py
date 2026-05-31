"""A one-tool MCP server exposing corpus-scoped search to the Tier B agent.

This is the airtight version of the agent's tool boundary. A Bash-command
wrapper cannot work: Claude Code's Bash allowlist is additive and does not
reliably restrict other commands, so an agent allowed `Bash(corpus-search:*)`
can still `cat` the corpus and read the answer without searching. The robust
fix is to expose search as an MCP tool and *deny* Bash (and the file-reading
tools) entirely, so the only way to learn anything about the corpus is to call
this tool.

The server is spawned by `claude -p` over stdio (see [`agent.py`]). It reads its
configuration from the environment so the config file carries no logic:

- ``SEARCH_EVAL_CORPUS``: the corpus directory to search (required).
- ``SEARCH_EVAL_SEARCH_BIN``: the `search` binary (default ``search``).
- ``SEARCH_EVAL_MAX_RESULTS``: results per query (default ``8``).
"""

from __future__ import annotations

import os
from pathlib import Path

from mcp.server.fastmcp import FastMCP

from .backend import SearchBackend

mcp = FastMCP("corpus")


def _backend() -> SearchBackend:
    corpus = os.environ.get("SEARCH_EVAL_CORPUS")
    if not corpus:
        raise RuntimeError("SEARCH_EVAL_CORPUS is not set")
    return SearchBackend(
        corpus=Path(corpus),
        search_bin=os.environ.get("SEARCH_EVAL_SEARCH_BIN", "search"),
        max_count=int(os.environ.get("SEARCH_EVAL_MAX_RESULTS", "8")),
    )


@mcp.tool()
def search(query: str) -> str:
    """Semantic search over the codebase. Returns matching files with snippets.

    This is the only way to read the codebase. Call it with a natural-language
    query describing what you are looking for.
    """
    hits = _backend().search(query, no_sync=True)
    if not hits:
        return "No results."
    return "\n\n".join(
        f"### {hit.path} (score {hit.score:.2f})\n{hit.text}" for hit in hits
    )


def main() -> None:
    mcp.run()


if __name__ == "__main__":
    main()
