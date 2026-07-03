import asyncio
import inspect
import pathlib
import sys

import pytest


sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "src" / "sh"))

import sh


def test_callable_module_signature_keeps_cmd_argument() -> None:
    assert "cmd" in inspect.signature(sh).parameters
    assert inspect.signature(sh) == inspect.signature(sh.sh)


def test_extra_positional_arg_gets_argv_hint() -> None:
    with pytest.raises(TypeError, match="argv as a single list"):
        sh("git", "status")


def test_failed_output_is_loud_at_both_ends() -> None:
    # Issue #1766: a dead build's Output rendered as ordinary text, so the
    # failure was indistinguishable from progress. The model view must lead
    # AND trail with the exit code, and the typed fields must be present.
    out = sh.Output(cmd="nix build .#mesa", code=3, raw="error: ENOSPC\n", duration=1204.2)
    rendered = repr(out)
    assert rendered.splitlines()[0].startswith("[exit 3]"), rendered
    assert "nix build .#mesa" in rendered.splitlines()[0], rendered
    assert rendered.rstrip().endswith("[exit 3]"), rendered
    assert out.exit_code == 3
    assert out.returncode == 3
    assert out.code == 3
    assert not out.ok
    assert bool(out) is False, "a failed Output must be falsy"
    # Diagnostics stay readable and marker-free: .text is the command's output.
    assert out.text == "error: ENOSPC\n"


def test_failed_output_with_no_output_still_renders_failure() -> None:
    out = sh.Output(cmd="false", code=1, raw="", duration=0.01)
    assert repr(out).startswith("[exit 1]"), repr(out)


def test_successful_output_is_truthy_even_when_empty() -> None:
    out = sh.Output(cmd="true", code=0, raw="", duration=0.01)
    assert out.ok
    assert bool(out) is True
    assert "[exit" not in repr(out)


def test_hint_rides_inside_the_failure_markers() -> None:
    # A hinted failure (e.g. a bare `grep` exiting 1) must still END with the
    # exit marker: the hint sits between the output and the trailing marker.
    out = sh.Output(cmd="grep foo bar.txt", code=1, raw="", duration=0.1, hint="use grep()")
    rendered = repr(out)
    assert rendered.splitlines()[0].startswith("[exit 1]"), rendered
    assert "[hint: use grep()]" in rendered, rendered
    assert rendered.rstrip().endswith("[exit 1]"), rendered


def test_long_command_is_truncated_in_failure_line() -> None:
    cmd = "nix build " + " ".join(f".#pkg{i}" for i in range(60))
    out = sh.Output(cmd=cmd, code=2, raw="boom\n", duration=1.0)
    first = repr(out).splitlines()[0]
    assert first.startswith("[exit 2]"), first
    assert first.endswith("..."), first
    assert len(first) < 200, first


def test_zsh_helper_uses_zsh_argv(monkeypatch: pytest.MonkeyPatch, tmp_path: pathlib.Path) -> None:
    seen = {}

    async def fake_sh(cmd: list[str], **kwargs: object) -> sh.Output:
        seen["cmd"] = cmd
        seen["kwargs"] = kwargs
        return sh.Output(cmd="zsh -lc print", code=0, raw="ok\n", duration=0)

    monkeypatch.setitem(sh.zsh.__globals__, "sh", fake_sh)

    cwd = str(tmp_path)
    out = asyncio.run(sh.zsh("print $ZSH_VERSION", cwd=cwd, timeout=1))

    assert out.ok
    assert seen == {
        "cmd": ["zsh", "-lc", "print $ZSH_VERSION"],
        "kwargs": {"cwd": cwd, "timeout": 1},
    }
