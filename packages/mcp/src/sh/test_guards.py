"""Regression tests for sh() input guards (ENG-2519).

These tests run standalone without the kernel runtime -- the module's graceful
fallback means `import sh` works fine outside the kernel environment.

Run with:
    python -m pytest packages/mcp/src/sh/test_guards.py -v
or:
    python packages/mcp/src/sh/test_guards.py
"""

import asyncio
import sys
import os

# Allow importing the sh package directly from its source directory.
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "sh"))
import sh as _sh_module

# sh() is the coroutine function inside the module; when the module is
# imported standalone _sh_module.sh is the async function.
sh = _sh_module.sh


def run(coro):
    return asyncio.run(coro)


# ---------------------------------------------------------------------------
# Backtick guard
# ---------------------------------------------------------------------------

def test_backtick_in_string_command_raises():
    """A string command containing a backtick must raise ValueError."""
    try:
        run(sh("git commit -m `some command`", cwd="."))
        assert False, "Expected ValueError"
    except ValueError as e:
        assert "backtick" in str(e).lower() or "command substitution" in str(e).lower(), str(e)


def test_backtick_in_repr_string_raises():
    """Simulates the exact failure mode: repr of a message containing backticks."""
    msg = "add `ix-mcp dashboard` support"
    cmd = f"git commit -m {msg!r}"
    # msg!r produces 'add `ix-mcp dashboard` support' -- backticks present
    assert "`" in cmd
    try:
        run(sh(cmd, cwd="."))
        assert False, "Expected ValueError"
    except ValueError as e:
        assert "backtick" in str(e).lower() or "command substitution" in str(e).lower(), str(e)


def test_backtick_in_argv_list_not_rejected():
    """argv-list form with backticks in an element must NOT be rejected (no shell parsing)."""
    msg = "add `ix-mcp dashboard` support"
    # This should run (and fail with non-zero because there's no git repo at /tmp,
    # but it must NOT raise ValueError before even starting the process).
    try:
        out = run(sh(["git", "commit", "-m", msg], cwd="/tmp"))
        # Non-zero exit is fine; we only care that no ValueError was raised.
        assert not out.ok or True
    except ValueError:
        raise AssertionError("argv-list form with backticks in an argument should not be rejected")
    except Exception:
        # Any other error (e.g. git not found, no repo) is acceptable here.
        pass


# ---------------------------------------------------------------------------
# Inline commit-message newline guard
# ---------------------------------------------------------------------------

def test_commit_message_with_escaped_newline_raises():
    r"""A git commit -m with a literal \n escape should raise ValueError."""
    try:
        run(sh(r"git commit -m 'subject\nbody'", cwd="."))
        assert False, "Expected ValueError"
    except ValueError as e:
        assert "newline" in str(e).lower() or "multi-line" in str(e).lower() or "-F" in str(e), str(e)


def test_commit_message_with_real_newline_raises():
    """A git commit -m with an embedded real newline should raise ValueError."""
    msg = "subject\nbody"
    try:
        run(sh(f"git commit -m '{msg}'", cwd="."))
        assert False, "Expected ValueError"
    except ValueError as e:
        assert "newline" in str(e).lower() or "multi-line" in str(e).lower() or "-F" in str(e), str(e)


def test_simple_commit_message_not_rejected():
    """A plain single-line git commit -m must NOT be rejected."""
    try:
        run(sh("git commit -m 'fix typo'", cwd="/tmp"))
    except ValueError:
        raise AssertionError("A simple single-line git commit -m should not be rejected")
    except Exception:
        # Non-zero exit or other runtime error is fine.
        pass


# ---------------------------------------------------------------------------
# cd guard (existing -- confirm still works)
# ---------------------------------------------------------------------------

def test_cd_prefix_still_rejected():
    """Existing cd-prefix guard must still raise ValueError."""
    try:
        run(sh("cd /tmp && ls", cwd="."))
        assert False, "Expected ValueError"
    except ValueError as e:
        assert "cwd=" in str(e), str(e)


# ---------------------------------------------------------------------------
# Benign string command still works
# ---------------------------------------------------------------------------

def test_benign_command_runs():
    """A simple string command with no backticks or newlines must run."""
    out = run(sh("echo hello", cwd="/tmp"))
    assert out.ok, f"Expected exit 0, got {out.code}"
    assert "hello" in out.text


if __name__ == "__main__":
    tests = [
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
        try:
            t()
            print(f"  PASS  {t.__name__}")
        except Exception as exc:
            print(f"  FAIL  {t.__name__}: {exc}")
            failed.append(t.__name__)
    if failed:
        print(f"\n{len(failed)} test(s) FAILED: {', '.join(failed)}")
        sys.exit(1)
    else:
        print(f"\nAll {len(tests)} tests passed.")
