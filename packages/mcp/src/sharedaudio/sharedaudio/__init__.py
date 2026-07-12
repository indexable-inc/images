"""Drive the local shared-audio daemon from a kernel cell.

Thin client for the daemon's unix control socket (JSON lines): check
status, adjust this machine's local volume, and publish WASM instruments
or control changes that every peer in the session picks up. Start the
daemon with `shared-audio daemon` (see packages/audio/README.md).
"""

from __future__ import annotations

import base64
import json
import os
import socket
from pathlib import Path
from typing import Any

__all__ = [
    "mute",
    "publish",
    "schedule",
    "set_control",
    "socket_path",
    "status",
    "unmute",
    "volume",
    "volume_down",
    "volume_up",
]


def socket_path() -> Path:
    """The daemon's control socket: `$SHARED_AUDIO_SOCKET` or the state dir."""
    override = os.environ.get("SHARED_AUDIO_SOCKET")
    if override:
        return Path(override)
    state = os.environ.get("XDG_STATE_HOME")
    base = Path(state) if state else Path.home() / ".local" / "state"
    return base / "shared-audio" / "control.sock"


def _request(payload: dict[str, Any]) -> dict[str, Any]:
    path = socket_path()
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
        try:
            sock.connect(str(path))
        except OSError as error:
            msg = f"connect {path} (is `shared-audio daemon` running?): {error}"
            raise ConnectionError(msg) from error
        sock.sendall(json.dumps(payload).encode() + b"\n")
        line = b""
        while not line.endswith(b"\n"):
            chunk = sock.recv(65536)
            if not chunk:
                break
            line += chunk
    reply: dict[str, Any] = json.loads(line.decode())
    if not reply.get("ok"):
        msg = f"daemon refused: {reply.get('error')}"
        raise RuntimeError(msg)
    return reply


def status() -> dict[str, Any]:
    """Daemon, clock, and score state (peer id, shared frame, controls...)."""
    reply = _request({"cmd": "status"})
    result: dict[str, Any] = reply["status"]
    return result


def volume(
    *,
    gain: float | None = None,
    step: float | None = None,
    muted: bool | None = None,
) -> None:
    """Adjust this machine's local volume. Never reaches the shared score."""
    payload: dict[str, Any] = {"cmd": "volume"}
    if gain is not None:
        payload["set"] = gain
    if step is not None:
        payload["step"] = step
    if muted is not None:
        payload["muted"] = muted
    _request(payload)


def volume_up(step: float = 0.1) -> None:
    """Raise local volume a step."""
    volume(step=step)


def volume_down(step: float = 0.1) -> None:
    """Lower local volume a step."""
    volume(step=-step)


def mute() -> None:
    """Silence local output; the session keeps playing for everyone else."""
    volume(muted=True)


def unmute() -> None:
    """Restore local output."""
    volume(muted=False)


def publish(module: str | Path | bytes, at_frame: int | None = None) -> None:
    """Publish a WASM (or WAT) instrument to every peer in the session.

    `module` is a path to a `.wasm`/`.wat` file, or the raw bytes. The
    daemon validates the module against the sa ABI before it can reach any
    peer. Everyone switches at `at_frame` (default: one second from now).
    """
    data = module if isinstance(module, bytes) else Path(module).read_bytes()
    payload: dict[str, Any] = {
        "cmd": "publish",
        "wasm_base64": base64.b64encode(data).decode(),
    }
    if at_frame is not None:
        payload["at_frame"] = at_frame
    _request(payload)


def set_control(control: int, value: float) -> None:
    """Set a shared instrument control for everyone, now."""
    _request({"cmd": "set_control", "control": control, "value": value})


def schedule(at_frame: int, control: int, value: float) -> None:
    """Schedule a shared control change at an exact shared frame."""
    _request(
        {"cmd": "schedule", "at_frame": at_frame, "control": control, "value": value}
    )
