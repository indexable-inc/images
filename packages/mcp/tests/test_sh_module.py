import asyncio
import inspect
import pathlib
import sys

import pytest


sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "src" / "sh"))

import sh  # noqa: E402


def test_callable_module_signature_keeps_cmd_argument() -> None:
    assert "cmd" in inspect.signature(sh).parameters
    assert inspect.signature(sh) == inspect.signature(sh.sh)


def test_extra_positional_arg_gets_argv_hint() -> None:
    with pytest.raises(TypeError, match="argv as a single list"):
        sh("git", "status")


def test_zsh_helper_uses_zsh_argv(monkeypatch: pytest.MonkeyPatch) -> None:
    seen = {}

    async def fake_sh(cmd: list[str], **kwargs: object) -> sh.Output:
        seen["cmd"] = cmd
        seen["kwargs"] = kwargs
        return sh.Output(cmd="zsh -lc print", code=0, raw="ok\n", duration=0)

    monkeypatch.setitem(sh.zsh.__globals__, "sh", fake_sh)

    out = asyncio.run(sh.zsh("print $ZSH_VERSION", cwd="/tmp", timeout=1))

    assert out.ok
    assert seen == {
        "cmd": ["zsh", "-lc", "print $ZSH_VERSION"],
        "kwargs": {"cwd": "/tmp", "timeout": 1},
    }
