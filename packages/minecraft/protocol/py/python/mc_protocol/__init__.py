"""Minecraft Java Edition Server List Ping.

The status exchange every Java Edition server answers on its game port: the
multiplayer screen's server list speaks it, and so do this repo's health
checks (mc-probe) and integration tests::

    import mc_protocol

    s = mc_protocol.status("localhost:25565", timeout_seconds=5.0)
    print(s.version_name, s.protocol_version)      # "26.1.2" 775
    print(s.players_online, s.players_max)         # 0 16
    print(mc_protocol.strip_format_codes(s.motd))  # codes removed
    print(s.latency_seconds)                       # ping/pong round-trip
    print(s.raw_json)                              # everything the server sent

The protocol implementation lives in the Rust ``mc-protocol`` crate
(packages/minecraft/protocol); this module is its unibind-rendered
binding, so Python, Rust, and any other bound language speak the wire format
through one implementation. Failures raise ``SlpError`` (an ``OSError``, the
family socket-level failures raise anyway) with ``InvalidInputError`` /
``NetworkError`` / ``ProtocolError`` subclasses per failure stage.
"""

from __future__ import annotations

from ._mc_protocol import (
    InvalidInputError,
    NetworkError,
    ProtocolError,
    ServerAddress,
    SlpError,
    SlpStatus,
    __version__,
    parse_address,
    status,
    strip_format_codes,
)

__all__ = [
    "InvalidInputError",
    "NetworkError",
    "ProtocolError",
    "ServerAddress",
    "SlpError",
    "SlpStatus",
    "__version__",
    "parse_address",
    "status",
    "strip_format_codes",
]
