"""Assert Minecraft Server List Ping responses.

An exit-code-driven CLI over the ``mc_protocol`` bindings (the Rust
``mc-protocol`` crate, packages/minecraft/protocol): a zero exit
means the server answered the SLP exchange and every requested assertion
held; any failure is named on stderr so health-check runners can surface it.

Designed for fleet health probes, not interactive inspection: the output is
intentionally terse and machine-friendly. Addresses are explicit
``host[:port]`` -- no SRV lookup (in-repo checks and tests address servers
directly; resolve SRV records before calling in).
"""

from __future__ import annotations

import argparse
import sys
from typing import TYPE_CHECKING

import mc_protocol

if TYPE_CHECKING:
    from collections.abc import Iterable


def _check_motd(status: mc_protocol.SlpStatus, needles: Iterable[str]) -> list[str]:
    plain = mc_protocol.strip_format_codes(status.motd)
    return [
        f"motd missing substring {needle!r} (got {plain!r})"
        for needle in needles
        if mc_protocol.strip_format_codes(needle) not in plain
    ]


def _check_protocol_version(
    status: mc_protocol.SlpStatus, expected: int | None
) -> list[str]:
    if expected is None or status.protocol_version == expected:
        return []
    return [
        f"protocol version {status.protocol_version} does not match expected {expected}"
    ]


def _check_max_players(status: mc_protocol.SlpStatus, minimum: int | None) -> list[str]:
    if minimum is None or status.players_max >= minimum:
        return []
    return [f"max players {status.players_max} below required {minimum}"]


def _parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        prog="mc-probe",
        description=__doc__.splitlines()[0] if __doc__ else None,
    )
    parser.add_argument(
        "address",
        help="Server address as host[:port] (default port 25565). No SRV lookup.",
    )
    parser.add_argument(
        "--motd-contains",
        action="append",
        default=[],
        metavar="SUBSTRING",
        help=(
            "Require the rendered MOTD to contain SUBSTRING. Color and format codes "
            "(both §X and &X spellings) are stripped from both sides before "
            "comparing. Repeatable."
        ),
    )
    parser.add_argument(
        "--protocol-version",
        type=int,
        metavar="N",
        help="Require the responding server to advertise protocol version N.",
    )
    parser.add_argument(
        "--min-max-players",
        type=int,
        metavar="N",
        help="Require the server's advertised max-player slot count to be at least N.",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=5.0,
        metavar="SECONDS",
        help="Connect+read timeout in seconds (default: 5).",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = _parse_args(sys.argv[1:] if argv is None else argv)

    try:
        status = mc_protocol.status(args.address, timeout_seconds=args.timeout)
    except mc_protocol.SlpError as exc:
        print(f"mc-probe: SLP failed for {args.address}: {exc}", file=sys.stderr)
        return 1

    failures: list[str] = []
    failures.extend(_check_motd(status, args.motd_contains))
    failures.extend(_check_protocol_version(status, args.protocol_version))
    failures.extend(_check_max_players(status, args.min_max_players))

    if failures:
        for failure in failures:
            print(f"mc-probe: {failure}", file=sys.stderr)
        return 1

    print(
        f"mc-probe: {args.address} ok "
        f"(version={status.version_name!r}, "
        f"protocol={status.protocol_version}, "
        f"players={status.players_online}/{status.players_max})"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
