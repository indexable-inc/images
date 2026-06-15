"""The requirements surface: local-only credential probes with exact remedies.

The contract defended here is the fail-fast UX: a missing credential is
reported with the env var, where to get a key, and the login alternative; a
present one names its *source* and never echoes a secret's value; and the
report's boolean is exactly "every declared credential resolves", which is what
`ix-mcp requirements` turns into its exit code.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from ix_notebook_mcp import registry, requirements


def _status(name: str) -> requirements.Status:
    return next(s for s in requirements.statuses() if s.name == name)


@pytest.fixture(autouse=True)
def no_ambient_credentials(monkeypatch, tmp_path):
    """No ambient credential can leak into a test: clear every declared env var
    and point HOME at an empty directory so token-path probes miss."""
    for _, credential in registry.credentialed():
        for var in credential.env:
            monkeypatch.delenv(var, raising=False)
    monkeypatch.setenv("HOME", str(tmp_path))


def test_missing_credential_remedy_names_every_route():
    # The line is the whole user experience of a missing credential: it must
    # carry the env var, where to get a key, and the login alternative, so the
    # user can fix it without reading docs.
    status = _status("search")
    assert status.satisfied_via is None
    for needle in ("MXBAI_API_KEY", "https://www.mixedbread.com", "`mgrep login`"):
        assert needle in status.line, status.line


def test_env_credential_names_the_var_not_the_value(monkeypatch):
    monkeypatch.setenv("EXA_API_KEY", "hunter2-super-secret")
    status = _status("exa_py")
    assert status.satisfied_via == "EXA_API_KEY"
    assert "hunter2-super-secret" not in status.line


def test_blank_env_value_is_not_a_credential(monkeypatch):
    monkeypatch.setenv("LINEAR_API_KEY", "   ")
    assert _status("linear").satisfied_via is None


def test_token_file_satisfies_when_env_is_absent(tmp_path):
    token = tmp_path / ".mgrep" / "token.json"
    token.parent.mkdir(parents=True)
    token.write_text("{}")
    assert _status("search").satisfied_via == "token at ~/.mgrep/token.json"


def test_report_emits_one_line_per_credential_and_gates_on_all(monkeypatch):
    emitted: list[str] = []
    assert requirements.report(emitted.append) is False
    assert len(emitted) == len(registry.credentialed())

    # Satisfy every credential (env where declared, token file otherwise) and
    # the report flips to True: the exact exit-code contract of the CLI.
    for _, credential in registry.credentialed():
        if credential.env:
            monkeypatch.setenv(credential.env[0], "configured")
        else:
            path = Path(credential.token_path).expanduser()
            path.parent.mkdir(parents=True, exist_ok=True)
            path.touch()
    assert requirements.report(lambda _line: None) is True
