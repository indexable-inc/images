"""Auto-loaded IPython startup for every ix-mcp notebook kernel.

The CLI copies this into ``$IPYTHONDIR/profile_default/startup/`` (see
``cli._prepare_ipython_startup``), so IPython runs it before the first cell of
every kernel. It turns on itables, which patches the IPython display formatter so
any pandas or polars ``DataFrame``/``Series`` renders as an interactive
DataTable (sort, search, paginate, horizontal scroll) with no per-session call.

``connected=True`` loads the DataTables library with an ESM ``import`` from the
jsDelivr CDN, which makes each table output self-contained. That is the mode that
works from a startup file: itables' offline mode embeds the library through the
*output* of ``init_notebook_mode``, so it must be called inside a real cell, not
here where the call's output has no cell to attach to.

itables keeps the ``text/plain`` repr alongside the HTML, so the MCP agent path
(which omits HTML) and any non-JS viewer still get a readable table; only the
human's JupyterLab view upgrades to the interactive widget.
"""

from itables import init_notebook_mode
import itables.options as it

# Render moderately large frames in full before itables downsamples to a slice
# with a banner. The default (64KB) clips common analysis frames; 256KB keeps
# them whole while still capping a runaway cell from embedding megabytes of JSON
# into the notebook (and the agent's context).
it.maxBytes = "256KB"

init_notebook_mode(all_interactive=True, connected=True)
