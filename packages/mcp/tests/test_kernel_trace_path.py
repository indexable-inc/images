"""index#2355: every serve pointed its kernel's faulthandler at one fixed
``kernel-trace.txt``, so concurrent kernels truncated and interleaved each
other's dumps and ``kernel_trace`` could return a different session's stacks.
The dump target must be per-serve, and files orphaned by SIGKILLed serves
must be swept at the next start."""

import os
import subprocess

import pytest

from ix_notebook_mcp.kernel import _sweep_stale_traces, trace_path_for


@pytest.fixture
def runtime(monkeypatch: pytest.MonkeyPatch, tmp_path_factory: pytest.TempPathFactory) -> str:
    base = tmp_path_factory.mktemp("xdg")
    monkeypatch.setenv("XDG_RUNTIME_DIR", str(base))
    return str(base)


def test_trace_path_is_per_server(runtime: str) -> None:
    a, b = trace_path_for(111), trace_path_for(222)
    assert a != b
    assert a.parent == b.parent
    assert str(a.parent).startswith(runtime)


def test_sweep_removes_only_dead_owners(runtime: str) -> None:
    dead = trace_path_for(_finished_pid())
    alive = trace_path_for(os.getpid())
    legacy = alive.parent / "kernel-trace.txt"  # older builds' fixed name
    junk = alive.parent / "kernel-trace-abc.txt"  # no pid suffix
    for p in (dead, alive, legacy, junk):
        p.write_text("dump")

    _sweep_stale_traces()

    assert not dead.exists()
    assert alive.exists()
    assert legacy.exists()
    assert junk.exists()


def _finished_pid() -> int:
    proc = subprocess.Popen(["true"])
    proc.wait()
    return proc.pid
