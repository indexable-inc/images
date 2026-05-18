"""Assert Minecraft Server List Ping responses.

Wraps :mod:`mcstatus` with an exit-code-driven CLI: a zero exit means the
server answered the SLP handshake and every requested assertion held; any
failure is named on stderr so health-check runners can surface it.

Designed for fleet health probes, not interactive inspection: the output is
intentionally terse and machine-friendly.
"""

from __future__ import annotations

import argparse
import re
import sys
from collections.abc import Iterable
from dataclasses import dataclass

from mcstatus import JavaServer
from mcstatus.responses import JavaStatusResponse

# Matches MC formatting codes in both common authoring conventions: the on-wire
# section-sign form (``§a``, U+00A7) and the ampersand source form (``&a``)
# that most server configs use because section signs are awkward to type.
_FORMAT_CODE = re.compile(r"[\u00a7&][0-9a-fk-orxA-FK-ORX]")


def _strip_formatting(text: str) -> str:
    return _FORMAT_CODE.sub("", text)


@dataclass(frozen=True)
class ProbeFailure:
    """A single assertion failure, ready to render."""

    message: str


def _check_motd(response: JavaStatusResponse, needles: Iterable[str]) -> list[ProbeFailure]:
    plain = _strip_formatting(response.motd.to_plain())
    return [
        ProbeFailure(f"motd missing substring {needle!r} (got {plain!r})")
        for needle in needles
        if _strip_formatting(needle) not in plain
    ]


def _check_protocol_version(
    response: JavaStatusResponse, expected: int | None
) -> list[ProbeFailure]:
    if expected is None or response.version.protocol == expected:
        return []
    return [
        ProbeFailure(
            f"protocol version {response.version.protocol} does not match expected {expected}"
        )
    ]


def _check_max_players(response: JavaStatusResponse, minimum: int | None) -> list[ProbeFailure]:
    if minimum is None or response.players.max >= minimum:
        return []
    return [ProbeFailure(f"max players {response.players.max} below required {minimum}")]


def _parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        prog="mc-probe",
        description=__doc__.splitlines()[0] if __doc__ else None,
    )
    parser.add_argument(
        "address",
        help="Server address as host[:port]. Resolves SRV records like the vanilla client.",
    )
    parser.add_argument(
        "--motd-contains",
        action="append",
        default=[],
        metavar="SUBSTRING",
        help=(
            "Require the rendered MOTD to contain SUBSTRING. Color and format codes "
            "(both \u00a7X and &X spellings) are stripped from both sides before "
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
        server = JavaServer.lookup(args.address, timeout=args.timeout)
        response = server.status()
    except Exception as exc:
        print(f"mc-probe: SLP failed for {args.address}: {exc}", file=sys.stderr)
        return 1

    failures: list[ProbeFailure] = []
    failures.extend(_check_motd(response, args.motd_contains))
    failures.extend(_check_protocol_version(response, args.protocol_version))
    failures.extend(_check_max_players(response, args.min_max_players))

    if failures:
        for failure in failures:
            print(f"mc-probe: {failure.message}", file=sys.stderr)
        return 1

    print(
        f"mc-probe: {args.address} ok "
        f"(version={response.version.name!r}, "
        f"protocol={response.version.protocol}, "
        f"players={response.players.online}/{response.players.max})"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
