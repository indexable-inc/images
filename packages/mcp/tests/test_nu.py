"""Behavior tests for the bundled `nu` module (the embedded nushell engine).

Everything drives the real in-process engine (the `nu._nu` PyO3 cdylib), so
what is defended is the module's contract, not a mock: the frame
normalization matrix, persistent REPL state, native datetime/duration
crossing, the df -> nu -> df roundtrip, the NuError diagnostic surface,
`exit` safety, and interrupt-based timeout.
"""

import asyncio
import datetime
import inspect
import pathlib

import polars as pl
import pytest

import nu


def run(coro: object) -> object:
    return asyncio.run(coro)


def test_callable_module_signature_keeps_code_argument() -> None:
    assert "code" in inspect.signature(nu).parameters
    assert inspect.signature(nu) == inspect.signature(nu.nu)


def test_table_becomes_frame() -> None:
    df = run(nu("[{a: 1, b: 'x'}, {a: 2, b: 'y'}] | where a > 1"))
    assert isinstance(df, pl.DataFrame)
    assert df.to_dicts() == [{"a": 2, "b": "y"}]


def test_record_becomes_one_row_frame() -> None:
    df = run(nu("{name: 'ix', stars: 7}"))
    assert df.shape == (1, 2)
    assert df["name"].item() == "ix"


def test_scalar_and_list_become_value_column() -> None:
    assert run(nu("2 + 2"))["value"].item() == 4
    assert run(nu("[1, 2, 3]"))["value"].to_list() == [1, 2, 3]


def test_null_and_empty_become_empty_frames() -> None:
    assert run(nu("null")).is_empty()
    assert run(nu("[] | where true")).is_empty()


def test_multi_statement_code_is_one_result() -> None:
    df = run(nu("let n = 3; seq 1 $n | each {|i| {n: $i, sq: ($i * $i)}}"))
    assert df["sq"].to_list() == [1, 4, 9]


def test_state_persists_across_calls_like_a_repl() -> None:
    run(nu("let repl_answer = 42"))
    run(nu("def double [x] { $x * 2 }"))
    assert run(nu.value("double $repl_answer")) == 84


def test_dataframe_roundtrip_through_pipeline() -> None:
    src = pl.DataFrame({"a": [1, 2, 3], "b": ["x", "y", "z"]})
    df = run(nu("where a > 1 | sort-by a --reverse", input=src))
    assert df.to_dicts() == [{"a": 3, "b": "z"}, {"a": 2, "b": "y"}]


def test_native_types_cross_exactly() -> None:
    df = run(nu("[{size: 1.5mb, dur: 3sec, when: 2024-01-02T03:04:05-05:00}]"))
    assert df["size"].item() == 1_500_000
    assert df.schema["dur"] == pl.Duration("us")
    assert df["dur"].item() == datetime.timedelta(seconds=3)
    when = df.schema["when"]
    assert isinstance(when, pl.Datetime)
    assert when.time_zone == "UTC"
    # -05:00 offsets normalize to one UTC timeline.
    assert df["when"].dt.hour().item() == 8


def test_error_carries_nushell_diagnostic() -> None:
    with pytest.raises(nu.NuError) as err:
        run(nu("[{a: 1}] | wherex a > 0"))
    message = str(err.value)
    assert "wherex" in message


def test_exit_raises_instead_of_killing_the_process() -> None:
    # eval_ir_block surfaces `exit` as an error; eval_block would have called
    # std::process::exit and taken the whole kernel down.
    with pytest.raises(nu.NuError):
        run(nu("exit 3"))
    # The engine is still usable afterwards.
    assert run(nu.value("1 + 1")) == 2


def test_value_escape_hatch_returns_plain_python() -> None:
    assert run(nu.value("{a: {b: [1, 2]}}")) == {"a": {"b": [1, 2]}}
    assert run(nu.value("'plain'")) == "plain"


def test_input_scalars_and_datetimes_cross_into_nu() -> None:
    stamp = datetime.datetime(2024, 1, 2, 3, 4, 5, tzinfo=datetime.UTC)
    assert run(nu.value("$in | format date '%Y'", input=stamp)) == "2024"
    assert run(nu.value("$in + 1", input=41)) == 42


def test_cwd_is_respected(tmp_path: pathlib.Path) -> None:
    (tmp_path / "hello.txt").write_text("hi")
    df = run(nu("ls | get name", cwd=tmp_path))
    assert df["value"].to_list() == ["hello.txt"]


def test_timeout_interrupts_a_runaway_pipeline() -> None:
    with pytest.raises(TimeoutError):
        run(nu("loop { }", timeout=0.5))
    # The interrupt leaves the engine healthy for the next call.
    assert run(nu.value("2 + 2")) == 4


def test_reset_discards_state() -> None:
    run(nu("let doomed = 1"))
    nu.reset()
    with pytest.raises(nu.NuError):
        run(nu("$doomed"))
