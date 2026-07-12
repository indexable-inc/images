"""Issue #2540 regressions: `input=` reaches the pipeline in multi-statement
code, or the call fails loudly -- never a silent drop.

`nu("cd /tmp; ^cat | complete", input="hi")` used to return empty stdout with
exit code 0: nushell block semantics feed input to the FIRST top-level
pipeline, and `cd` (declared `nothing` input) drained the payload. Real
casualties: `gh issue comment --body-file -` posted "Body cannot be blank"
and `git commit --file -` aborted on an empty message. Drives the real
embedded engine (the `nu._nu` PyO3 cdylib), like test_nu.py.
"""

import asyncio
import pathlib
import sys

import pytest

import nu

# An external that echoes its stdin, using the interpreter binary the sandbox
# certainly has (the pattern test_nu.py uses for external-command coverage).
STDIN_ECHO = f'^{sys.executable} -c "import sys; sys.stdout.write(sys.stdin.read())"'


def run(coro: object) -> object:
    return asyncio.run(coro)


def test_input_reaches_external_stdin_after_cd(tmp_path: pathlib.Path) -> None:
    # The issue's literal shape: a `cd` prefix must not eat the external's stdin.
    try:
        rec = run(nu(f"cd {tmp_path}; {STDIN_ECHO} | complete", input="hi"))
        assert isinstance(rec, dict)
        assert rec["exit_code"] == 0
        assert rec["stdout"] == "hi"
    finally:
        nu.reset()


def test_input_routes_to_mid_block_external_not_the_last_pipeline(
    tmp_path: pathlib.Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    # Routing targets the first ACCEPTING pipeline, not blindly the final one:
    # the mid-block external gets the payload, its output prints as an
    # intermediate (issue #2391), and the final statement's value returns.
    # (An INTERNAL command that cannot take `nothing` input is already a parse
    # error mid-block, so externals are the consumers this routing serves.)
    try:
        assert run(nu(f"cd {tmp_path}; {STDIN_ECHO}; 'done'", input="hi")) == "done"
        assert "hi" in capsys.readouterr().out
    finally:
        nu.reset()


def test_first_accepting_pipeline_still_receives_input(
    capsys: pytest.CaptureFixture[str],
) -> None:
    # Unchanged pre-#2540 behavior when the first pipeline CAN consume: input
    # feeds it (nushell block semantics) and its output prints as an
    # intermediate (issue #2391) while the final value is returned.
    assert run(nu("str upcase; 'done'", input="hi")) == "done"
    assert "HI" in capsys.readouterr().out


def test_undeliverable_input_raises_instead_of_dropping(tmp_path: pathlib.Path) -> None:
    try:
        with pytest.raises(nu.NuError, match="input="):
            run(nu(f"cd {tmp_path}; cd {tmp_path}", input="hi"))
    finally:
        nu.reset()


def test_single_no_input_statement_raises(tmp_path: pathlib.Path) -> None:
    try:
        with pytest.raises(nu.NuError, match="input="):
            run(nu(f"cd {tmp_path}", input="hi"))
    finally:
        nu.reset()
