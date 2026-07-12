"""Regression tests for the kernel process runner's input guards (ENG-2519).

The public ``sh()``/``zsh()`` are retired (agents shell out through ``await
nu(...)``) and now raise a migration hint; the guards live on the private
``_exec`` runner the kernel's own internals still use. These tests run
standalone without the kernel runtime -- the module's graceful fallback means
``import sh`` works fine outside the kernel environment.

Run with:
    python -m pytest packages/mcp/src/sh/test_guards.py -v
or:
    python packages/mcp/src/sh/test_guards.py
"""

from __future__ import annotations

import asyncio
import sys
from pathlib import Path

import pytest

# Allow importing the sh package directly from its source directory.
sys.path.insert(0, str(Path(__file__).parent / "sh"))
import sh as _sh_module

# The guards live on the private runner `_exec`; the public `sh`/`zsh` are the
# disabled shims tested separately below.
exec_ = _sh_module._exec


def run(coro: object) -> object:
    return asyncio.run(coro)  # type: ignore[arg-type]


# ---------------------------------------------------------------------------
# Public entry points are disabled
# ---------------------------------------------------------------------------

def test_public_sh_raises_migration_hint() -> None:
    """`sh()` is retired; calling it raises with a pointer to `await nu(...)`."""
    with pytest.raises(RuntimeError, match=r"await nu"):
        run(_sh_module.sh("echo hi"))


def test_public_zsh_raises_migration_hint() -> None:
    with pytest.raises(RuntimeError, match=r"await nu"):
        run(_sh_module.zsh("echo hi"))


def test_calling_module_raises_migration_hint() -> None:
    with pytest.raises(RuntimeError, match=r"await nu"):
        run(_sh_module("echo hi"))


# ---------------------------------------------------------------------------
# Backtick guard (on the private runner)
# ---------------------------------------------------------------------------

def test_backtick_in_string_command_raises() -> None:
    """A string command containing a backtick must raise ValueError."""
    with pytest.raises(ValueError, match=r"(?i)backtick|command substitution"):
        run(exec_("git commit -m `some command`", cwd="."))


def test_backtick_in_repr_string_raises() -> None:
    """Simulates the exact failure mode: repr of a message containing backticks."""
    msg = "add `ix-mcp dashboard` support"
    cmd = f"git commit -m {msg!r}"
    # msg!r produces 'add `ix-mcp dashboard` support' -- backticks present
    assert "`" in cmd
    with pytest.raises(ValueError, match=r"(?i)backtick|command substitution"):
        run(exec_(cmd, cwd="."))


def test_backtick_in_argv_list_not_rejected() -> None:
    """argv-list form with backticks in an element must NOT be rejected (no shell parsing)."""
    msg = "add `ix-mcp dashboard` support"
    # This should run (and fail with non-zero because there's no git repo at /tmp,
    # but it must NOT raise ValueError before even starting the process).
    try:
        run(exec_(["git", "commit", "-m", msg], cwd="/tmp"))  # noqa: S108 -- test-only; /tmp is intentional
    except ValueError:
        raise AssertionError("argv-list form with backticks in an argument should not be rejected") from None
    except Exception:  # noqa: S110 -- non-ValueError errors (git not found, no repo) are acceptable in this test
        pass


# ---------------------------------------------------------------------------
# Inline commit-message newline guard
# ---------------------------------------------------------------------------

def test_commit_message_with_escaped_newline_raises() -> None:
    r"""A git commit -m with a literal \n escape should raise ValueError."""
    with pytest.raises(ValueError, match=r"(?i)newline|multi-line|-F"):
        run(exec_(r"git commit -m 'subject\nbody'", cwd="."))


def test_commit_message_with_real_newline_raises() -> None:
    """A git commit -m with an embedded real newline should raise ValueError."""
    msg = "subject\nbody"
    with pytest.raises(ValueError, match=r"(?i)newline|multi-line|-F"):
        run(exec_(f"git commit -m '{msg}'", cwd="."))


def test_simple_commit_message_not_rejected() -> None:
    """A plain single-line git commit -m must NOT be rejected."""
    try:
        run(exec_("git commit -m 'fix typo'", cwd="/tmp"))  # noqa: S108 -- test-only; /tmp is intentional
    except ValueError:
        raise AssertionError("A simple single-line git commit -m should not be rejected") from None
    except Exception:  # noqa: S110 -- non-zero exit or other runtime error (no git repo) is fine in this test
        pass


# ---------------------------------------------------------------------------
# cd guard (existing -- confirm still works)
# ---------------------------------------------------------------------------

def test_cd_prefix_still_rejected() -> None:
    """Existing cd-prefix guard must still raise ValueError."""
    with pytest.raises(ValueError, match="cwd="):
        run(exec_("cd /tmp && ls", cwd="."))


# ---------------------------------------------------------------------------
# Benign string command still works
# ---------------------------------------------------------------------------

def test_benign_command_runs() -> None:
    """A simple string command with no backticks or newlines must run."""
    out = run(exec_("echo hello", cwd="/tmp"))  # noqa: S108 -- test-only; /tmp is intentional
    assert out.ok, f"Expected exit 0, got {out.code}"  # type: ignore[union-attr]
    assert "hello" in out.text  # type: ignore[union-attr]


def _run_tests() -> None:
    tests = [
        test_public_sh_raises_migration_hint,
        test_public_zsh_raises_migration_hint,
        test_calling_module_raises_migration_hint,
        test_backtick_in_string_command_raises,
        test_backtick_in_repr_string_raises,
        test_backtick_in_argv_list_not_rejected,
        test_commit_message_with_escaped_newline_raises,
        test_commit_message_with_real_newline_raises,
        test_simple_commit_message_not_rejected,
        test_cd_prefix_still_rejected,
        test_benign_command_runs,
    ]
    failed = []
    for t in tests:
        ok = True
        try:
            t()
        except Exception as exc:
            print(f"  FAIL  {t.__name__}: {exc}")
            ok = False
        if ok:
            print(f"  PASS  {t.__name__}")
        else:
            failed.append(t.__name__)
    if failed:
        print(f"\n{len(failed)} test(s) FAILED: {', '.join(failed)}")
        sys.exit(1)
    else:
        print(f"\nAll {len(tests)} tests passed.")


if __name__ == "__main__":
    _run_tests()
