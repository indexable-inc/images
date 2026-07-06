"""Regression tests for the kernel runner's name= parameter (ENG-2486).

The public ``sh()`` is retired; the runner is the private ``_exec``, which the
kernel's own internals (grep/find, worktree) call. These tests run standalone
without the kernel runtime.

Run with:
    python packages/mcp/src/sh/test_name_param.py
"""

from __future__ import annotations

import asyncio
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent / "sh"))
import sh as _sh_module

sh = _sh_module._exec


def run(coro: object) -> object:
    return asyncio.run(coro)  # type: ignore[arg-type]


def test_name_accepted_standalone() -> None:
    """sh() must accept name= outside the kernel (no TypeError)."""
    out = run(sh("echo hi", name="my-label"))
    assert out.ok, f"Expected exit 0, got {out.code}"  # type: ignore[union-attr]
    assert "hi" in out.text  # type: ignore[union-attr]


def test_name_none_accepted() -> None:
    """name=None (the default) must work without error."""
    out = run(sh("echo ok", name=None))
    assert out.ok  # type: ignore[union-attr]


def test_name_does_not_affect_output() -> None:
    """name= must not alter the command output."""
    out = run(sh("echo world", name="test-job"))
    assert "world" in out.text  # type: ignore[union-attr]


def test_name_with_other_kwargs() -> None:
    """name= must compose cleanly with other keyword arguments."""
    out = run(sh("echo combined", cwd="/tmp", timeout=10, name="combo"))  # noqa: S108 -- test-only; /tmp is intentional
    assert out.ok  # type: ignore[union-attr]


def test_rename_current_job_none_outside_kernel() -> None:
    """_rename_current_job must be None (not importable) outside the kernel."""
    # The module exposes _rename_current_job as None when the runtime is absent.
    import sh as sh_mod

    rename_fn = getattr(sh_mod, "_rename_current_job", "MISSING")
    # It should be set to None (the fallback), not a real function.
    assert rename_fn is None, (
        f"Expected _rename_current_job=None outside kernel, got {rename_fn!r}"
    )


def _run_tests() -> None:
    tests = [
        test_name_accepted_standalone,
        test_name_none_accepted,
        test_name_does_not_affect_output,
        test_name_with_other_kwargs,
        test_rename_current_job_none_outside_kernel,
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
