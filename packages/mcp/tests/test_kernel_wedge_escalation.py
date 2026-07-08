"""python_exec wedge escalation: interrupt delivery is not recovery.

index#2375: a cell that blocks the kernel's main thread inside native code
(the embedded nu engine, index#2095; any C extension) never reaches a bytecode
boundary, so the Python-level SIGUSR2 rescue handler cannot run. The old path
still reported "interrupted and is usable again" on mere signal delivery and
left the kernel wedged until the client SIGTERMed the whole serve
(index#2365). python_exec must instead verify the rescue with a probe and,
when the probe also hangs, kill and respawn only the kernel child
(``restart_now``) so the next call runs.
"""

import asyncio
import os
import tempfile
from pathlib import Path

from ix_notebook_mcp import cli
from ix_notebook_mcp.config import Config
from ix_notebook_mcp.kernel import Kernel


def _config() -> Config:
    # Install the shipped IPython startup so the in-kernel runtime (__ix_exec,
    # Result, the SIGUSR1/SIGUSR2 handlers) loads in the booted kernel, as the
    # CLI wires it.
    os.environ["IPYTHONDIR"] = str(cli._prepare_ipython_startup(0))
    return Config(workdir=Path(tempfile.mkdtemp()), wedge_grace=1.0, max_budget=5.0)


def test_unrescuable_block_escalates_to_kernel_restart() -> None:
    config = _config()

    async def main() -> None:
        kernel = Kernel(config)
        await kernel.start()
        try:
            # Stand in for a native-code block: SIG_IGN makes the process drop
            # SIGUSR2 at the C level, exactly like a main thread stuck inside a
            # Rust call that never reaches a bytecode boundary -- the signal is
            # delivered but the Python-level rescue never runs, so the sleep
            # holds the loop for its full duration.
            _, armed = await kernel.python_exec(
                "import signal\nsignal.signal(signal.SIGUSR2, signal.SIG_IGN)\nResult.ok('armed')",
                budget=15.0,
                name="arm",
            )
            assert armed is not None
            assert armed["status"] == "done", armed
            old_pid = kernel._pid
            assert old_pid is not None

            _, summary = await kernel.python_exec("import time\ntime.sleep(120)", budget=0.5, name="block")
            assert summary is not None
            assert summary["status"] == "wedged", summary
            assert "killed and respawned" in summary["error"], summary
            # The escalation bounced only this kernel child; the fresh pid proves it.
            assert kernel._pid is not None
            assert kernel._pid != old_pid, (old_pid, kernel._pid)
            assert kernel._death is None, kernel._death

            _, after = await kernel.python_exec("Result.text('alive')", budget=10.0, name="after")
            assert after is not None
            assert after["status"] == "done", after
        finally:
            await kernel.shutdown()

    asyncio.run(main())


def test_rescued_block_reports_verified_recovery() -> None:
    config = _config()

    async def main() -> None:
        kernel = Kernel(config)
        await kernel.start()
        try:
            old_pid = kernel._pid
            # A plain Python-level block: SIGUSR2 interrupts it, the probe
            # confirms the kernel answered, and NO restart happens.
            _, summary = await kernel.python_exec("import time\ntime.sleep(120)", budget=0.5, name="block")
            assert summary is not None
            assert summary["status"] == "wedged", summary
            assert "usable again" in summary["error"], summary
            assert "asyncio.to_thread" in summary["error"], summary
            assert kernel._pid == old_pid, (old_pid, kernel._pid)

            _, after = await kernel.python_exec("Result.text('alive')", budget=10.0, name="after")
            assert after is not None
            assert after["status"] == "done", after
        finally:
            await kernel.shutdown()

    asyncio.run(main())
