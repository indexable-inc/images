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


def test_lone_string_round_trips_as_plain_text() -> None:
    # Issue #2068: a lone string is text, not data -- framing it as a 1x1
    # DataFrame made every print of it show polars' width-clipped box repr,
    # and the full multiline text was unrecoverable from the captured stdout.
    lines = [f"---VERIFY {i}: a fairly long line of stdout text, step {i} in detail---" for i in range(10)]
    joined = " ".join(f"'{line}'" for line in lines)
    text = run(nu(f"[{joined}] | str join (char nl)"))
    assert isinstance(text, str)
    assert text.splitlines() == lines


def test_external_stdout_is_the_full_string() -> None:
    # The reported shape (issue #2068): `^cmd` multiline stdout must come back
    # verbatim as str, not as a frame that clips at repr time.
    import sys

    script = "print(chr(10).join('section %d: ' % i + 'x' * 60 for i in range(10)))"
    out = run(nu(f'^{sys.executable} -c "{script}"'))
    assert isinstance(out, str)
    body = out.strip().splitlines()
    assert body == [f"section {i}: " + "x" * 60 for i in range(10)]


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


def test_int_list_input_stays_a_list_not_binary() -> None:
    # extract::<Vec<u8>> would have eaten [1, 2, 3] as binary.
    assert run(nu.value("$in | math sum", input=[1, 2, 3])) == 6


def test_bytes_input_arrives_as_binary() -> None:
    assert run(nu.value("$in | decode", input=b"hi")) == "hi"


def test_oversized_int_input_errors_instead_of_rounding() -> None:
    with pytest.raises(nu.NuError, match="out of range"):
        run(nu.value("$in", input=2**80))


def test_mixed_type_results_still_frame() -> None:
    assert run(nu("[1, 2.5]"))["value"].to_list() == [1.0, 2.5]
    df = run(nu("[{a: 1}, {a: 'x'}]"))
    assert df.height == 2


def test_trailing_external_output_is_collected(tmp_path: pathlib.Path) -> None:
    # Stack::collect_value(): a bare external at the end of the pipeline must
    # come back as the value, not leak to the host process stdout (which under
    # MCP stdio transport is the protocol stream). `nu --testbin cococo` is a
    # cross-platform echo shipped inside the nushell binary itself... which the
    # embedded engine does not have on PATH; use the interpreter binary we
    # certainly have: python3 printing a marker.
    import sys

    out = run(nu.value(f"^{sys.executable} -c 'print(\"collected\")'"))
    assert isinstance(out, str)
    assert out.strip() == "collected"


def test_naive_datetime_input_gets_a_clear_error() -> None:
    naive = datetime.datetime(2024, 1, 2, 3, 4, 5)  # noqa: DTZ001 -- naive on purpose: it IS the case under test
    with pytest.raises(nu.NuError, match="naive datetime"):
        run(nu.value("$in", input=naive))


def test_empty_record_is_one_row_zero_columns() -> None:
    # Pins the degenerate corner of the record -> 1-row contract so a polars
    # behavior change is caught here, not by a confused caller.
    df = run(nu("{}"))
    assert df.shape == (1, 0)


def test_cd_does_not_outlive_its_call(tmp_path: pathlib.Path) -> None:
    # PWD re-syncs to the process cwd on every call (index#1986/#1999): a
    # `cd` in one call must not leak into the next, and deleting a dir a
    # previous call cd'ed into must not break later calls.
    target = tmp_path / "transient"
    target.mkdir()
    run(nu(f"cd {target}"))
    assert run(nu.value("$env.PWD")) == str(pathlib.Path.cwd())
    run(nu(f"cd {target}"))
    target.rmdir()
    assert run(nu.value("2 + 2")) == 4


def test_cwd_is_respected(tmp_path: pathlib.Path) -> None:
    (tmp_path / "hello.txt").write_text("hi")
    df = run(nu("ls | get name", cwd=tmp_path))
    assert df["value"].to_list() == ["hello.txt"]


def test_timeout_interrupts_and_discards_engine_state() -> None:
    run(nu("let survivor = 'no'"))
    with pytest.raises(TimeoutError):
        run(nu("loop { }", timeout=0.5))
    # The next call gets a FRESH engine (a stuck element could hold the old
    # one indefinitely), so it works immediately but persistent state is gone.
    assert run(nu.value("2 + 2")) == 4
    with pytest.raises(nu.NuError):
        run(nu("$survivor"))


def test_reset_discards_state() -> None:
    run(nu("let doomed = 1"))
    nu.reset()
    with pytest.raises(nu.NuError):
        run(nu("$doomed"))


def test_nu_registers_job_resource(monkeypatch: pytest.MonkeyPatch) -> None:
    class Job:
        id = "job456"

    class Current:
        def get(self) -> Job:
            return Job()

    class Resource:
        def __init__(self) -> None:
            self.closed = False

        def close(self) -> None:
            self.closed = True

    calls: list[dict[str, object]] = []
    resource = Resource()

    def register_resource(**kwargs: object) -> Resource:
        calls.append(kwargs)
        return resource

    monkeypatch.setattr(nu, "_ix_current", Current())
    monkeypatch.setattr(nu, "_register_resource", register_resource)
    monkeypatch.setattr(nu, "_resource_counts", {})

    df = run(nu("1 + 1"))

    assert df["value"].item() == 2
    assert resource.closed
    assert len(calls) == 1
    call = calls[0]
    assert call["id"] == "nu-job456-1"
    assert call["kind"] == "nu"
    assert str(call["title"]).startswith("nu: ")
    render = call["render"]
    assert callable(render)
    html = render()
    assert "done" in html
    assert "2" in html
    alive = call["alive"]
    assert callable(alive)
    assert alive() is False


def test_externals_run_color_free_even_when_host_forces_color(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # issue #2051: the kernel process typically runs with color FORCED by its
    # launcher (FORCE_COLOR=1 / CLICOLOR_FORCE=1), which the engine used to
    # inherit wholesale, so JSON-mode CLIs (`gh ... --json`) emitted
    # ANSI-wrapped JSON into a captured pipe and `from json` choked. The engine
    # copies the host env at construction, so force color first, then build a
    # fresh engine.
    import sys

    monkeypatch.setenv("FORCE_COLOR", "1")
    monkeypatch.setenv("CLICOLOR", "1")
    monkeypatch.setenv("CLICOLOR_FORCE", "1")
    monkeypatch.setenv("GH_FORCE_TTY", "100%")
    nu.reset()
    # A color-happy external: wraps its JSON in SGR exactly when the env asks
    # for color (the same decision gh makes). chr(27) keeps the script free of
    # backslashes so it survives nushell's double-quote escaping untouched.
    script = (
        "import json, os, sys;"
        "force = os.environ.get('CLICOLOR_FORCE', '0') not in ('', '0')"
        " or os.environ.get('FORCE_COLOR', '0') not in ('', '0');"
        "on = force and not os.environ.get('NO_COLOR');"
        "body = json.dumps({'state': 'MERGED'});"
        "esc = chr(27);"
        "sys.stdout.write(esc + '[1;37m' + body + esc + '[m' if on else body)"
    )
    try:
        env = run(nu.value("$env | select -o NO_COLOR CLICOLOR CLICOLOR_FORCE FORCE_COLOR"))
        assert env == {
            "NO_COLOR": "1",
            "CLICOLOR": "0",
            "CLICOLOR_FORCE": "0",
            "FORCE_COLOR": "0",
        }
        # GH_FORCE_TTY (TTY-style gh rendering into a pipe) must not cross over.
        assert run(nu.value("'GH_FORCE_TTY' in $env")) is False
        df = run(nu(f'^{sys.executable} -c "{script}" | from json'))
        assert df["state"].item() == "MERGED"
        # env= still re-enables color for the one call that wants raw ANSI.
        raw = run(
            nu.value(
                f'^{sys.executable} -c "{script}"',
                env={"NO_COLOR": "", "CLICOLOR_FORCE": "1"},
            )
        )
        assert "\x1b[" in raw
    finally:
        # The forced-color engine (and the env= override, which persists on
        # the stack) must not leak into later tests.
        nu.reset()
