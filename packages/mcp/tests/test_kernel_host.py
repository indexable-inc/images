"""The kernel host seam: where the kernel process lives (kernel_host.py).

Selection must be explicit (``Config.kernel_host`` -> ``make_kernel_host``,
unknown values loud), and the Ray host must rebuild a working ZMQ client from
the plain-dict connection info the actor ships back (str HMAC key, node-IP
endpoints) -- the piece that lets exec traffic flow server -> kernel directly
and keeps Ray as control plane only. None of this needs a running Ray: the
Ray-touching paths import ray lazily, inside actor spawn.
"""

import asyncio
import tempfile
from pathlib import Path

import pytest

from ix_notebook_mcp.config import Config
from ix_notebook_mcp.kernel import Kernel
from ix_notebook_mcp.kernel_host import (
    KernelActor,
    LocalKernelHost,
    RayKernelHost,
    make_kernel_host,
)


def test_selection_is_explicit_and_unknown_is_loud() -> None:
    assert isinstance(make_kernel_host("local"), LocalKernelHost)
    assert isinstance(make_kernel_host("ray"), RayKernelHost)
    with pytest.raises(ValueError, match="unknown kernel host"):
        make_kernel_host("k8s")


def test_config_defaults_to_local_child() -> None:
    # An embedder building Config directly (the tests, the notebook engine)
    # must keep today's behavior with no Ray anywhere near the process.
    config = Config(workdir=Path(tempfile.gettempdir()))
    assert config.kernel_host == "local"
    assert isinstance(Kernel(config)._host, LocalKernelHost)


def test_kernel_binds_the_configured_host_without_importing_ray() -> None:
    # Constructing a ray-hosted Kernel must be side-effect free: ray connects
    # at start(), never at __init__ (the CLI builds Config long before serve).
    config = Config(workdir=Path(tempfile.gettempdir()), kernel_host="ray")
    assert isinstance(Kernel(config)._host, RayKernelHost)


def test_ray_client_rebuilds_from_shipped_connection_info() -> None:
    # The actor ships a plain dict (str key); the server-side client must come
    # back HMAC-keyed and pointed at the kernel node's routable endpoints, or
    # every execute would silently talk to nothing.
    host = RayKernelHost()
    host._apply(
        {
            "pid": 4242,
            "node_ip": "100.64.0.7",
            "connection": {
                "transport": "tcp",
                "ip": "100.64.0.7",
                "shell_port": 51001,
                "iopub_port": 51002,
                "stdin_port": 51003,
                "hb_port": 51004,
                "control_port": 51005,
                "signature_scheme": "hmac-sha256",
                "key": "5ecret",
            },
        }
    )
    kc = host.client()
    assert host.pid == 4242
    assert kc.ip == "100.64.0.7"
    assert kc.session.key == b"5ecret"
    assert kc.session.signature_scheme == "hmac-sha256"
    assert (kc.shell_port, kc.iopub_port, kc.control_port) == (51001, 51002, 51005)


def test_actor_info_ships_a_json_safe_key() -> None:
    # jupyter's get_connection_info returns the HMAC key as bytes; the actor
    # payload keeps it a str so the dict stays JSON-safe for logs/facts, and
    # load_connection_info on the server side re-encodes it (pinned above).
    class FakeKM:
        ip = "100.64.0.7"

        def get_connection_info(self, *, session: bool = False) -> dict:
            assert session is False
            return {"transport": "tcp", "ip": self.ip, "key": b"5ecret"}

    actor = KernelActor()
    actor._core._km = FakeKM()
    actor._core._pid = 7
    info = actor._info()
    assert info == {
        "pid": 7,
        "node_ip": "100.64.0.7",
        "connection": {"transport": "tcp", "ip": "100.64.0.7", "key": "5ecret"},
    }


def test_local_trace_reads_are_offset_scoped(tmp_path: Path) -> None:
    # dump_trace's contract: mark the size, signal, read only what was appended
    # past the mark. The host primitives carry that offset arithmetic.
    host = LocalKernelHost()
    host._trace_path = tmp_path / "kernel-trace-1.txt"

    async def main() -> None:
        assert await host.trace_size() == 0  # absent file reads as empty
        host._trace_path.write_text("first dump\n")
        mark = await host.trace_size()
        host._trace_path.write_text("first dump\nsecond dump\n")
        assert await host.trace_read(mark) == "second dump\n"

    asyncio.run(main())
