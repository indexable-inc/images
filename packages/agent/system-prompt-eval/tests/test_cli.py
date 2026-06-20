"""Offline, deterministic checks for the CLI surface.

Runnable as a plain script (``python tests/test_cli.py``) so the Nix build can
exercise it against the installed package with no test runner, no network, and no
API key. These guard the ``--quiet`` wiring: the flag parses, and it routes
progress to a silent sink instead of the stderr ticker.
"""

from __future__ import annotations

import io
from contextlib import redirect_stderr

from system_prompt_eval.cli import _build_parser, _make_progress, _progress
from system_prompt_eval.core import noop


def test_quiet_defaults_off() -> None:
    args = _build_parser().parse_args(["run"])
    assert args.quiet is False


def test_quiet_flag_parses() -> None:
    args = _build_parser().parse_args(["run", "--quiet"])
    assert args.quiet is True


def test_make_progress_quiet_is_noop() -> None:
    # Quiet must reuse the shared silent sink, not a fresh stub.
    assert _make_progress(quiet=True) is noop
    assert _make_progress(quiet=False) is _progress


def test_quiet_sink_emits_nothing() -> None:
    buf = io.StringIO()
    with redirect_stderr(buf):
        _make_progress(quiet=True)("hello")
    assert buf.getvalue() == "", "quiet progress must print nothing to stderr"


def test_loud_sink_emits_to_stderr() -> None:
    buf = io.StringIO()
    with redirect_stderr(buf):
        _make_progress(quiet=False)("hello")
    assert "hello" in buf.getvalue(), "default progress must print to stderr"


def _main() -> None:
    tests = [v for name, v in sorted(globals().items()) if name.startswith("test_")]
    for test in tests:
        test()
    print(f"ok: {len(tests)} cli tests passed")


if __name__ == "__main__":
    _main()
