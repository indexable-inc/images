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


def test_record_becomes_plain_dict() -> None:
    # Issue #2390: a record is a struct, not a table. Framing it as a 1-row
    # DataFrame forced every field read through `df.to_dicts()[0]`, and the
    # natural `d['exit_code']` / `d.get('stderr')` on a `| complete` result
    # failed with "'DataFrame' object has no attribute 'get'".
    rec = run(nu("{name: 'ix', stars: 7}"))
    assert rec == {"name": "ix", "stars": 7}


def test_complete_record_fields_read_directly() -> None:
    # The dominant record producer (issue #2390): `do -i { ^cmd } | complete`
    # captures stdout/stderr/exit_code of a fallible external; each field must
    # read straight off the returned dict.
    import sys

    script = "import sys; print('kept'); sys.stderr.write('boom'); raise SystemExit(3)"
    rec = run(nu(f'do -i {{ ^{sys.executable} -c "{script}" }} | complete'))
    assert isinstance(rec, dict)
    assert rec["exit_code"] == 3
    assert str(rec["stdout"]).strip() == "kept"
    assert "boom" in str(rec["stderr"])


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


def test_intermediate_pipeline_output_prints_instead_of_dropping(
    capsys: pytest.CaptureFixture[str],
) -> None:
    # Issue #2391: a multi-statement cell used to return only the last
    # pipeline's value and silently drop everything before it (an agent read
    # `git show | to text; git status | to text` as empty commits). Now each
    # non-final pipeline's output prints into the captured stdout while the
    # final pipeline's value stays the return value.
    result = run(nu("'first' | str upcase; [1 2 3]; 'final'"))
    assert result == "final"
    printed = capsys.readouterr().out
    assert "FIRST" in printed
    assert "value" in printed  # the [1 2 3] intermediate prints as a frame


def test_single_statement_prints_nothing_extra(capsys: pytest.CaptureFixture[str]) -> None:
    assert run(nu("'only'")) == "only"
    assert capsys.readouterr().out == ""


def test_silent_intermediates_stay_silent(capsys: pytest.CaptureFixture[str]) -> None:
    # `let` produces no output; printing blank lines for it would be noise.
    assert run(nu.value("let quiet = 1; $quiet + 1")) == 2
    assert capsys.readouterr().out == ""


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


def test_check_false_keeps_output_and_surfaces_exit_code() -> None:
    # The whole point of check=False (index#2067): the output the external
    # produced before exiting non-zero must survive, alongside its exit code
    # (check=True drops the collected output on the NuError path). The output
    # is a lone string, so it stays plain text (index#2068 semantics).
    import sys

    script = "print('kept'); raise SystemExit(3)"
    res = run(nu(f'^{sys.executable} -c "{script}"', check=False))
    assert isinstance(res, nu.NuResult)
    result, exit_code = res
    assert exit_code == 3
    assert isinstance(result, str)
    assert result.strip() == "kept"


def test_check_false_grep_no_match_is_empty_not_an_error() -> None:
    # grep exits 1 on "no match", which is an answer, not a failure.
    import sys

    script = "raise SystemExit(1)"
    result, exit_code = run(nu(f'^{sys.executable} -c "{script}"', check=False))
    assert exit_code == 1
    assert result == ""


def test_check_false_success_reports_exit_code_zero() -> None:
    res = run(nu("2 + 2", check=False))
    assert isinstance(res, nu.NuResult)
    assert res.exit_code == 0
    assert isinstance(res.result, pl.DataFrame)
    assert res.result["value"].item() == 4


def test_check_true_default_still_raises_on_non_zero_exit() -> None:
    import sys

    with pytest.raises(nu.NuError, match="non-zero exit code"):
        run(nu(f"^{sys.executable} -c 'raise SystemExit(3)'"))


def test_check_false_still_raises_on_real_errors() -> None:
    # Only exit-status semantics are relaxed; a broken pipeline is still an
    # exception either way.
    with pytest.raises(nu.NuError):
        run(nu("[{a: 1}] | wherex a > 0", check=False))



def test_big_external_output_inside_try_does_not_deadlock() -> None:
    # index#2015: nushell's experimental `pipefail` (the 0.113 default) made
    # try/catch collection wait on the child's exit status BEFORE draining its
    # stdout. A child with more pending output than the OS pipe buffer
    # (64 KiB) then never exits -- it blocks in write(2) and no EPIPE arrives
    # because the engine still holds the read end -- so the eval hung forever
    # and wedged the engine for every later call. The bindings disable
    # pipefail, so this must complete and hand back all the output.
    import sys

    async def guarded() -> object:
        writer = f"^{sys.executable} -c 'import sys; sys.stdout.write(\"x\"*130000)'"
        # wait_for is a tripwire, not part of the contract: on regression this
        # fails in seconds instead of hanging the test run forever.
        return await asyncio.wait_for(
            nu.value("try { " + writer + " } catch { 'caught' }"),
            timeout=60,
        )

    out = run(guarded())
    assert out == "x" * 130_000


def test_failing_external_raises_and_try_catches_without_pipefail() -> None:
    # Disabling pipefail must not cost error reporting: a trailing external's
    # non-zero exit still raises through ByteStream's consume-then-wait check,
    # and try/catch still routes it to the catch block.
    import sys

    with pytest.raises(nu.NuError, match="non-zero exit code"):
        run(nu(f"^{sys.executable} -c 'raise SystemExit(7)'"))
    caught = run(
        nu.value("try { ^" + sys.executable + " -c 'raise SystemExit(7)' } catch { 'caught' }")
    )
    assert caught == "caught"


def test_naive_datetime_input_gets_a_clear_error() -> None:
    naive = datetime.datetime(2024, 1, 2, 3, 4, 5)  # noqa: DTZ001 -- naive on purpose: it IS the case under test
    with pytest.raises(nu.NuError, match="naive datetime"):
        run(nu.value("$in", input=naive))


def test_empty_record_is_empty_dict() -> None:
    # Pins the degenerate corner of the record -> dict contract (issue #2390).
    assert run(nu("{}")) == {}


def test_cd_persists_across_calls(tmp_path: pathlib.Path) -> None:
    # Issue #2089: the per-call re-sync of PWD to the process cwd (the kernel's
    # launch dir -- typically another agent's worktree) silently redirected
    # bare `git` commands across worktrees. PWD is REPL state like `let`/`def`:
    # a `cd` outlives its call, per engine (and engines are per session).
    target = tmp_path / "workdir"
    target.mkdir()
    try:
        run(nu(f"cd {target}"))
        assert pathlib.Path(run(nu.value("$env.PWD"))).resolve() == target.resolve()
    finally:
        nu.reset()


def test_removed_cwd_fails_loudly_and_cwd_recovers(tmp_path: pathlib.Path) -> None:
    # Issue #1986's failure mode, now with a diagnosis and a remedy instead of
    # a cryptic engine error -- and never a silent redirect somewhere else.
    target = tmp_path / "transient"
    target.mkdir()
    try:
        run(nu(f"cd {target}"))
        target.rmdir()
        with pytest.raises(nu.NuError, match="no longer exists"):
            run(nu.value("2 + 2"))
        # An explicit cwd= both runs the call and repairs the engine.
        keep = tmp_path / "keep"
        keep.mkdir()
        assert run(nu.value("2 + 2", cwd=keep)) == 4
        assert pathlib.Path(run(nu.value("$env.PWD"))).resolve() == keep.resolve()
    finally:
        nu.reset()


def test_explicit_cwd_persists_like_cd(tmp_path: pathlib.Path) -> None:
    try:
        run(nu.value("2 + 2", cwd=tmp_path))
        assert pathlib.Path(run(nu.value("$env.PWD"))).resolve() == tmp_path.resolve()
    finally:
        nu.reset()


def test_nonexistent_explicit_cwd_is_rejected_at_the_boundary(tmp_path: pathlib.Path) -> None:
    with pytest.raises(ValueError, match="not a directory"):
        run(nu.value("2 + 2", cwd=tmp_path / "missing"))


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
        env = run(
            nu.value(
                "$env | select -o NO_COLOR CLICOLOR CLICOLOR_FORCE FORCE_COLOR GH_PROMPT_DISABLED"
            )
        )
        assert env == {
            "NO_COLOR": "1",
            "CLICOLOR": "0",
            "CLICOLOR_FORCE": "0",
            "FORCE_COLOR": "0",
            # issue #2163: gh must never try to prompt into a captured pipe.
            "GH_PROMPT_DISABLED": "1",
        }
        # GH_FORCE_TTY (TTY-style gh rendering into a pipe) must not cross over.
        assert run(nu.value("'GH_FORCE_TTY' in $env")) is False
        rec = run(nu(f'^{sys.executable} -c "{script}" | from json'))
        assert rec == {"state": "MERGED"}
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
