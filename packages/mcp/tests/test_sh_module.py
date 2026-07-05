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


def test_failed_output_with_no_output_still_leads_and_trails() -> None:
    # Even with no output the model text both leads with the failure line and
    # ends with the bare marker, so tail-reads see the terminal state.
    out = sh.Output(cmd="false", code=1, raw="", duration=0.01)
    rendered = repr(out)
    assert rendered.startswith("[exit 1]"), rendered
    assert rendered.rstrip().endswith("\n[exit 1]"), rendered


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


# NOTE: no test below embeds anything resembling a real credential; every
# fixture token is constructed at test time from repeated filler (the same
# convention as packages/search/source/meta/src/sanitize.rs tests).


def test_failure_line_redacts_bearer_token() -> None:
    token = "tok9" * 10
    out = sh.Output(cmd=f'curl -H "Authorization: Bearer {token}" https://api.example.com', code=7, raw="", duration=0.5)
    first = repr(out).splitlines()[0]
    assert token not in repr(out), repr(out)
    assert "[redacted:" in first, first
    # argv[0] stays, so the command is still identifiable.
    assert "curl" in first, first


def test_failure_line_redacts_credential_kwargs_keeping_key() -> None:
    secret = "hunter" + "2" * 20
    out = sh.Output(cmd=f"deploy --host h1 password={secret} token={secret}", code=1, raw="", duration=0.1)
    rendered = repr(out)
    assert secret not in rendered, rendered
    assert "password=[redacted:credential]" in rendered, rendered
    assert "token=[redacted:credential]" in rendered, rendered


def test_failure_line_redacts_known_key_prefixes_and_blobs() -> None:
    gh = "ghp_" + "Ab1" * 12
    blob = "QUJD/0+=" * 40
    out = sh.Output(cmd=f"gh api -H 'X-Tok: {gh}' --input {blob}", code=1, raw="", duration=0.1)
    rendered = repr(out)
    assert gh not in rendered, rendered
    assert blob not in rendered, rendered
    assert "[redacted:github_token]" in rendered, rendered
    assert "[blob 320 chars]" in rendered, rendered


def test_shell_error_message_is_redacted() -> None:
    token = "tok9" * 10
    out = sh.Output(cmd=f"curl -H 'Authorization: Bearer {token}'", code=22, raw="", duration=0.2)
    err = sh.ShellError(out)
    assert token not in str(err), str(err)
    assert "[redacted:" in str(err), str(err)
    # The Output the error carries keeps the raw command for programmatic use.
    assert err.output.cmd == out.cmd


def test_multiline_command_collapses_to_one_failure_line() -> None:
    cmd = "set -e\nfor f in a b c; do\n  build $f\ndone"
    out = sh.Output(cmd=cmd, code=2, raw="", duration=0.3)
    rendered = repr(out)
    first = rendered.splitlines()[0]
    assert first.startswith("[exit 2]"), rendered
    assert "for f in a b c; do build $f done" in first, first
    assert rendered.rstrip().endswith("[exit 2]"), rendered


def test_redaction_does_not_touch_executed_command_or_text() -> None:
    token = "tok9" * 10
    out = sh.Output(cmd=f"echo token={token}", code=1, raw=f"token={token}\n", duration=0.1)
    # .cmd and .text stay raw (programmatic surfaces); only renders scrub.
    assert token in out.cmd
    assert token in out.text


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


def test_sh_registers_job_resource(monkeypatch: pytest.MonkeyPatch) -> None:
    class Job:
        id = "job123"

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

    monkeypatch.setattr(sh, "_ix_current", Current())
    monkeypatch.setattr(sh, "_register_resource", register_resource)
    monkeypatch.setattr(sh, "_resource_counts", {})

    out = asyncio.run(sh.sh([sys.executable, "-c", "print('resource-ok')"], echo=False))

    assert out.ok
    assert resource.closed
    assert len(calls) == 1
    call = calls[0]
    assert call["id"] == "sh-job123-1"
    assert call["kind"] == "sh"
    assert str(call["title"]).startswith("sh: ")
    assert callable(call["render"])
    html = call["render"]()
    assert "resource-ok" in html
    assert "done" in html
    alive = call["alive"]
    assert callable(alive)
    assert alive() is False


def test_sh_startup_failure_registers_terminal_resource(monkeypatch: pytest.MonkeyPatch) -> None:
    class Job:
        id = "job404"

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

    monkeypatch.setattr(sh, "_ix_current", Current())
    monkeypatch.setattr(sh, "_register_resource", register_resource)
    monkeypatch.setattr(sh, "_resource_counts", {})

    with pytest.raises(FileNotFoundError):
        asyncio.run(sh.sh(["__ix_missing_executable_for_resource_test__"], echo=False))

    assert resource.closed
    assert len(calls) == 1
    call = calls[0]
    assert call["id"] == "sh-job404-1"
    render = call["render"]
    assert callable(render)
    html = render()
    assert "FileNotFoundError" in html
    assert "failed" in html
