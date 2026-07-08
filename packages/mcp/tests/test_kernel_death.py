"""An externally killed kernel must be loud and precise, then heal itself.

index#2339: a broad ``pkill -f ipykernel_launcher`` SIGTERM'd a session's kernel
child and the server presented it as a generic ``status: wedged`` timeout,
``kernel_trace`` signalled the zombie and blamed a missing faulthandler, and the
kernel only came back on the next execute. The death watch must instead report
``kernel died (pid N, signal S); respawning`` (in-flight and subsequent calls
alike), say the process is gone in ``kernel_trace``, log the death to stderr
(which reaches journald), and respawn without waiting for a client call.
"""

import asyncio
import os
import signal
import tempfile
from pathlib import Path

import pytest

from ix_notebook_mcp import cli
from ix_notebook_mcp.config import Config
from ix_notebook_mcp.kernel import Kernel, KernelDiedError


def test_killed_kernel_reports_death_and_respawns(capsys: pytest.CaptureFixture[str]) -> None:
    # Install the shipped IPython startup so the in-kernel runtime (__ix_exec,
    # Result, the signal handlers) loads in the booted kernel, as the CLI wires it.
    os.environ["IPYTHONDIR"] = str(cli._prepare_ipython_startup(0))
    config = Config(workdir=Path(tempfile.mkdtemp()), wedge_grace=1.0, max_budget=5.0)

    async def main() -> None:
        kernel = Kernel(config)
        await kernel.start()
        try:
            loop = asyncio.get_running_loop()
            _, up = await kernel.python_exec("Result.ok('up')", budget=15.0, name="up")
            assert up is not None, up
            assert up["status"] == "done", up
            pid = kernel._pid
            assert pid is not None

            # Kill the kernel child with a request in flight: the call must fail
            # promptly with the precise cause, not sit out budget+grace and
            # come back as a generic 'wedged' summary.
            inflight = asyncio.ensure_future(
                kernel.python_exec("await asyncio.sleep(60)\nResult.ok('nope')", budget=30.0, name="inflight")
            )
            await asyncio.sleep(1.0)  # the execute request is on the wire
            started = loop.time()
            os.kill(pid, signal.SIGTERM)
            with pytest.raises(KernelDiedError) as err:
                await inflight
            elapsed = loop.time() - started
            assert f"kernel died (pid {pid}, signal SIGTERM); respawning" in str(err.value), str(err.value)
            assert elapsed < 15, ("death was reported at the wedge timeout, not eagerly", elapsed)

            # While the respawn is still in progress a NEW call gets the same
            # precise error (the respawn needs a fresh process + ready handshake,
            # so it cannot have finished within this same event-loop breath) ...
            with pytest.raises(KernelDiedError) as during:
                await kernel.python_exec("Result.ok('during')", budget=5.0, name="during")
            assert f"pid {pid}" in str(during.value), str(during.value)

            # ... and kernel_trace says the process is gone, instead of signalling
            # the corpse and blaming a missing faulthandler.
            trace = await kernel.dump_trace()
            assert "kernel process is gone" in trace, trace
            assert f"pid {pid}" in trace, trace

            # The watch respawns on its own: the death flag clears and the pid
            # changes WITHOUT this test issuing any execute.
            for _ in range(1200):  # up to ~120s; a fresh kernel boots in a few
                if kernel._death is None:
                    break
                await asyncio.sleep(0.1)
            assert kernel._death is None, kernel._death
            assert kernel._pid is not None, kernel._pid
            assert kernel._pid != pid, (kernel._pid, pid)

            # The respawned kernel serves cells again.
            _, back = await kernel.python_exec("Result.ok('back')", budget=15.0, name="back")
            assert back is not None, back
            assert back["status"] == "done", back
        finally:
            await kernel.shutdown()

    asyncio.run(main())
    # The death must be loud: both the death and the respawn reach stderr (journald).
    err_text = capsys.readouterr().err
    assert "kernel died (pid" in err_text, err_text
    assert "signal SIGTERM" in err_text, err_text
    assert "kernel respawned (pid" in err_text, err_text
