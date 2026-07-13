"""Nested command results remain terminal, bounded, and structured (#3079)."""

from __future__ import annotations

import asyncio

import polars as pl
import pytest

import nu
from ix_notebook_mcp import runtime


def _wire(monkeypatch: pytest.MonkeyPatch, ns: dict[str, object]) -> None:
    monkeypatch.setattr(runtime, "_user_ns", ns)
    monkeypatch.setattr(runtime, "_baseline_names", frozenset(ns))
    monkeypatch.setattr(runtime, "_session_namespaces", {})
    monkeypatch.setattr(runtime, "_typecheck_enabled", lambda: False)


async def _run_and_parse() -> tuple[runtime.Job, object]:
    job = await runtime.__ix_run("results", budget=5.0)
    parsed = await nu.value("from nuon", input=job.result.llm_result)
    return job, parsed


def _parse(text: str) -> object:
    return asyncio.run(nu.value("from nuon", input=text))


def test_nested_nu_dataframes_finish_with_named_output(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    results = [
        nu.NuResult(pl.DataFrame({"number": [1], "title": ["one"]}), 0),
        nu.NuResult(pl.DataFrame({"number": [2], "title": ["two"]}), 0),
    ]
    _wire(monkeypatch, {"results": results})

    job, parsed = asyncio.run(_run_and_parse())

    assert job.status == "done", (job.status, job.error)
    assert job.result.value is results
    assert [row["exit_code"] for row in parsed] == [0, 0]
    assert '[[number, title]; [1, "one"]]' in parsed[0]["result"]
    assert '[[number, title]; [2, "two"]]' in parsed[1]["result"]
    assert job.result.user_html.count('data-ix-field="result"') == 2
    assert job.result.user_html.count('data-ix-field="exit_code"') == 2


def test_plain_nu_results_also_keep_their_named_structure(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    results = [
        nu.NuResult("hello", 0),
        nu.NuResult({"nested": {"answer": 42}}, 3),
    ]
    _wire(monkeypatch, {"results": results})

    job, parsed = asyncio.run(_run_and_parse())

    assert job.status == "done", (job.status, job.error)
    assert parsed == [
        {"result": "hello", "exit_code": 0},
        {"result": {"nested": {"answer": 42}}, "exit_code": 3},
    ]


def test_nested_dataframe_uses_its_bounded_render_once(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    extra_rows = 5
    frame = pl.DataFrame(
        {"number": list(range(runtime._DF_LLM_ROWS + extra_rows))}
    )
    results = [nu.NuResult(frame, 0)]
    original = runtime._df_llm_text
    calls = 0

    def counted(value: object) -> str:
        nonlocal calls
        calls += 1
        return original(value)

    monkeypatch.setattr(runtime, "_df_llm_text", counted)
    _wire(monkeypatch, {"results": results})

    job, parsed = asyncio.run(_run_and_parse())

    assert job.status == "done", (job.status, job.error)
    assert calls == 1
    assert f"... ({extra_rows} more rows)" in parsed[0]["result"]
    assert f"[{runtime._DF_LLM_ROWS - 1}]" in parsed[0]["result"]
    assert f"[{runtime._DF_LLM_ROWS}]" not in parsed[0]["result"]


class _RenderedOnly:
    def __ix_html__(self) -> str:
        return "<strong>bounded</strong>"

    def __ix_llm__(self) -> str:
        return "bounded"

    def __repr__(self) -> str:
        return "RAW_VALUE_MUST_NOT_RENDER"


def test_nested_leaf_uses_its_render_instead_of_raw_repr(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    results = [nu.NuResult(_RenderedOnly(), 0)]
    _wire(monkeypatch, {"results": results})

    job, parsed = asyncio.run(_run_and_parse())

    assert job.status == "done", (job.status, job.error)
    assert parsed == [{"result": "bounded", "exit_code": 0}]
    assert "RAW_VALUE_MUST_NOT_RENDER" not in job.result.llm_result


def test_nested_container_cycles_and_depth_are_bounded() -> None:
    cycle: list[object] = []
    cycle.append(cycle)
    rendered_cycle = runtime.Result.of({"cycle": cycle, "result": nu.NuResult("ok", 0)})
    assert "<cycle to list>" in rendered_cycle.llm_result

    deep: list[object] = []
    cursor = deep
    for _ in range(runtime._NESTED_RENDER_MAX_DEPTH + 2):
        child: list[object] = []
        cursor.append(child)
        cursor = child
    cursor.append(nu.NuResult("too deep", 0))
    rendered_deep = runtime.Result.of(deep)
    assert (
        f"omitted at nested render depth {runtime._NESTED_RENDER_MAX_DEPTH}"
        in rendered_deep.llm_result
    )


def test_nested_sequence_and_mapping_caps_are_explicit() -> None:
    limit = runtime._NESTED_RENDER_MAX_ITEMS
    sequence: list[object] = [nu.NuResult("first", 0), *range(limit)]
    rendered_sequence = runtime.Result.of(sequence)
    parsed_sequence = _parse(rendered_sequence.llm_result)
    assert len(parsed_sequence) == limit + 1
    assert parsed_sequence[0] == {"result": "first", "exit_code": 0}
    assert parsed_sequence[-1] == {"truncated": 1}
    assert "1 more items" in rendered_sequence.user_html

    mapping: dict[str, object] = {f"key_{index}": index for index in range(limit)}
    mapping["rich_after_limit"] = nu.NuResult("omitted safely", 0)
    rendered_mapping = runtime.Result.of(mapping)
    parsed_mapping = _parse(rendered_mapping.llm_result)
    assert parsed_mapping["truncated"] == 1
    assert len(parsed_mapping["items"]) == limit
    assert "rich_after_limit" not in parsed_mapping["items"]
    assert "1 more entries" in rendered_mapping.user_html


class _RendererPanic(BaseException):
    pass


class _PanickingValue:
    def __ix_llm__(self) -> str:
        raise _RendererPanic("renderer panic")


def test_renderer_base_exception_terminalizes_the_job(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _wire(monkeypatch, {"panic": _PanickingValue()})

    job = asyncio.run(runtime.__ix_run("panic", budget=5.0))

    assert job.status == "error"
    assert job.done()
    assert "renderer panic" in (job.error or "")

    async def await_job() -> object:
        return await job

    with pytest.raises(_RendererPanic, match="renderer panic"):
        asyncio.run(await_job())
