"""Regression tests for sh() name= parameter (ENG-2486).

These tests run standalone without the kernel runtime.

Run with:
    python packages/mcp/src/sh/test_name_param.py
"""

import asyncio
import sys
import os

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "sh"))
import sh as _sh_module

sh = _sh_module.sh


def run(coro):
    return asyncio.run(coro)


def test_name_accepted_standalone():
    """sh() must accept name= outside the kernel (no TypeError)."""
    out = run(sh("echo hi", name="my-label"))
    assert out.ok, f"Expected exit 0, got {out.code}"
    assert "hi" in out.text


def test_name_none_accepted():
    """name=None (the default) must work without error."""
    out = run(sh("echo ok", name=None))
    assert out.ok


def test_name_does_not_affect_output():
    """name= must not alter the command output."""
    out = run(sh("echo world", name="test-job"))
    assert "world" in out.text


def test_name_with_other_kwargs():
    """name= must compose cleanly with other keyword arguments."""
    out = run(sh("echo combined", cwd="/tmp", timeout=10, name="combo"))
    assert out.ok


def test_rename_current_job_none_outside_kernel():
    """_rename_current_job must be None (not importable) outside the kernel."""
    # The module exposes _rename_current_job as None when the runtime is absent.
    import sh as sh_mod

    rename_fn = getattr(sh_mod, "_rename_current_job", "MISSING")
    # It should be set to None (the fallback), not a real function.
    assert rename_fn is None, (
        f"Expected _rename_current_job=None outside kernel, got {rename_fn!r}"
    )


if __name__ == "__main__":
    tests = [
        test_name_accepted_standalone,
        test_name_none_accepted,
        test_name_does_not_affect_output,
        test_name_with_other_kwargs,
        test_rename_current_job_none_outside_kernel,
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
