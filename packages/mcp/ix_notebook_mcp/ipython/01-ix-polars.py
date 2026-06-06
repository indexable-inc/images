"""Widen polars' text repr for the MCP agent's view of DataFrames.

The dashboard renders a DataFrame result from its ``text/html`` repr, so this
only changes the ``text/plain`` repr: what the agent (which reads text, not HTML)
and any non-JS viewer get. Polars defaults truncate to ~8 rows, ~8 columns, and
30-char strings, which hides most of a frame from the agent; widen to a fuller
but still bounded view. The MCP layer caps a single text output at 50k chars
(outputs._MAX_TEXT_CHARS), so a wide repr cannot flood the agent's context.
"""

import polars as pl

pl.Config.set_tbl_rows(40)
pl.Config.set_tbl_cols(40)
pl.Config.set_fmt_str_lengths(80)
pl.Config.set_tbl_width_chars(160)
