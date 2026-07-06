"""A 1x1 string DataFrame renders its text verbatim to the model (issue #1976).

``nu()`` frames every scalar, so ``await nu("^cat Cargo.toml")`` is a 1x1
string frame; the model text used to be one JSON-escaped NUON cell the agent
could only read by re-fetching with ``.item()``.
"""

from __future__ import annotations

import polars as pl

from ix_notebook_mcp import runtime


def test_scalar_string_frame_renders_verbatim() -> None:
    content = '[package]\nname = "ix"\nversion = "1.9.0"\n'
    text = runtime.Result.of(pl.DataFrame({"value": [content]})).llm_result
    # The shape header still orients the reader; the body is the raw string --
    # real newlines, no JSON escaping, no NUON table brackets.
    assert text == f"shape: (1, 1) | value:String\n{content}"


def test_scalar_string_frame_strips_terminal_escapes() -> None:
    text = runtime.Result.of(pl.DataFrame({"value": ["\x1b[31mred\x1b[0m plain"]})).llm_result
    assert text.endswith("\nred plain")


def test_scalar_non_string_frame_keeps_the_nuon_table() -> None:
    text = runtime.Result.of(pl.DataFrame({"value": [4]})).llm_result
    assert text == "shape: (1, 1) | value:Int64\n[[value]; [4]]"


def test_single_row_multi_column_frame_keeps_the_nuon_table() -> None:
    text = runtime.Result.of(pl.DataFrame({"a": ["x"], "b": ["y"]})).llm_result
    assert '[[a, b]; ["x", "y"]]' in text
