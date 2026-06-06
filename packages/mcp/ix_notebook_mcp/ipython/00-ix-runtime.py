"""Auto-loaded IPython startup: install the ix-mcp in-kernel runtime.

The CLI copies this into ``$IPYTHONDIR/profile_default/startup/`` so IPython runs
it before the first cell of the one shared kernel. It installs ``jobs``/``Job``/
``__ix_exec`` into the user namespace and wires per-job output capture and the
SQLite store. Named ``00-`` so it runs before the itables/polars tweaks.
"""

import sys

try:
    from ix_notebook_mcp.runtime import install

    install(get_ipython().user_ns)  # noqa: F821  (get_ipython is injected by IPython)
except Exception as exc:  # pragma: no cover - defensive: a broken runtime must be loud
    print(f"[ix-mcp] runtime install failed: {exc!r}", file=sys.stderr)
