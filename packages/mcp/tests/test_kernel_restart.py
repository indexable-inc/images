"""kernel_restart: an intentional, per-server kernel restart must be surgical.

index#2345: recovering ONE wedged kernel required `pkill -f ipykernel_launcher`,
which SIGTERM'd every session's kernel on the machine. The `kernel_restart`
tool's primitive (``Kernel.restart_now``) must instead bounce only this server's
kernel child: the pid changes, the namespace is rebuilt, the session name/topic
the server had pushed are re-applied to the fresh process, stderr carries the
requested-restart lines (which reach journald) and NEVER the death watch's
``kernel died`` report (the kill is intentional, so the watch from index#2344
is cancelled around it, exactly as ``shutdown()`` does), and the watch is
re-armed for the new process afterwards.
"""

import asyncio
import os
import tempfile
from pathlib import Path

import pytest

from ix_notebook_mcp import cli
from ix_notebook_mcp.config import Config
from ix_notebook_mcp.kernel import Kernel


def test_restart_now_is_intentional_and_scoped(capsys: pytest.CaptureFixture[str]) -> None:
    # Install the shipped IPython startup so the in-kernel runtime (__ix_exec,
    # Result, session) loads in the booted kernel, as the CLI wires it.
    os.environ["IPYTHONDIR"] = str(cli._prepare_ipython_startup(0))
    config = Config(workdir=Path(tempfile.mkdtemp()), wedge_grace=1.0, max_budget=5.0)

    async def main() -> None:
        kernel = Kernel(config)
        await kernel.start()
        try:
            await kernel.set_session_name("restart smoke")
            await kernel.set_topic("restart topic")
            _, up = await kernel.python_exec("marker = 41\nResult.ok('up')", budget=15.0, name="up")
            assert up is not None, up
            assert up["status"] == "done", up
            old_pid = kernel._pid
            assert old_pid is not None

            info = await kernel.restart_now()

            # The restart is reported precisely: old pid, new pid, elapsed time.
            assert info["old_pid"] == old_pid, info
            assert info["new_pid"] == kernel._pid, info
            assert info["new_pid"] != old_pid, info
            assert isinstance(info["elapsed_s"], float), info
            assert info["elapsed_s"] >= 0, info
            # Usable immediately: no lingering death flag, and the death watch is
            # re-armed for the new process.
            assert kernel._death is None, kernel._death
            assert kernel._watch_task is not None
            assert not kernel._watch_task.done()

            # Give a mis-cancelled death watch several poll intervals to
            # (wrongly) report the intentional kill; the stderr assertions at the
            # end then catch it.
            await asyncio.sleep(2.0)

            # The namespace was rebuilt (no session file, so nothing restores
            # `marker`), but the session name/topic the server pushed survive.
            _, after = await kernel.python_exec(
                "try:\n"
                "    marker\n"
                "    found = True\n"
                "except NameError:\n"
                "    found = False\n"
                "print(f'found={found} name={session.name} topic={session.topic}')\n"
                "Result.ok('after')",
                budget=15.0,
                name="after",
            )
            assert after is not None, after
            assert after["status"] == "done", after
            text = f"{after.get('output') or ''}\n{after.get('result') or ''}"
            assert "found=False" in text, text
            assert "name=restart smoke" in text, text
            assert "topic=restart topic" in text, text
        finally:
            await kernel.shutdown()

    asyncio.run(main())
    err_text = capsys.readouterr().err
    # Loud and intentional on stderr (journald): the requested-restart lines ...
    assert "kernel restart requested (pid" in err_text, err_text
    assert "kernel restarted on request (pid" in err_text, err_text
    # ... and never the death watch's unexpected-death report or respawn line.
    assert "kernel died" not in err_text, err_text
    assert "kernel respawned" not in err_text, err_text
