#!/usr/bin/env python3
"""Clear cookies for blocked domains across Chromium-based browsers.

Fed the firewall blocklist (see flake.nix `blockedHosts`) on every
home-manager switch, so a DNS-blocked site is also a logged-out site: the
moment a domain enters the blocklist its cookies are purged, and they stay
purged for as long as it is blocked. Unblock + switch and the domain drops off
the list, so you start fresh on next login.

Two strategies, because a Chromium cookie store behaves differently depending
on whether the browser is running:

- Running browser with a remote-debugging port (Dia ships one on 9223): clear
  live over the Chrome DevTools Protocol. This is the only reliable path while
  the browser holds the Cookies DB open under PRAGMA locking_mode=EXCLUSIVE and
  keeps cookies cached in memory (flushed every 30s/512 ops, so a direct
  sqlite write would be clobbered on exit).
- Closed browser: DELETE rows straight from the Cookies SQLite store. host_key
  is stored in plaintext, so no profile-key decryption is needed.

stdlib only (sqlite3 + a ~40-line websocket client) so it carries no Python
deps into the home-manager closure.
"""

import base64
import json
import os
from pathlib import Path
import socket
import sqlite3
import struct
import sys
from urllib import error, parse, request


def matches(host_key: str, bases: list[str]) -> bool:
    """A cookie host (possibly leading-dot, e.g. ".x.com") belongs to a base
    domain when it equals the base or is a subdomain of it. Suffix logic, not
    substring: "washingtonpost.com" must not match the base "t.co"."""
    host = host_key.removeprefix(".")
    host = host.lower()
    return any(host == base or host.endswith("." + base) for base in bases)


# ---------------------------------------------------------------------------
# Closed browsers: direct SQLite delete.
# ---------------------------------------------------------------------------

# macOS Application Support roots for the Chromium family. Cookies live at
# <profile>/Cookies (older) or <profile>/Network/Cookies (current).
CHROMIUM_ROOTS = [
    "Google/Chrome",
    "Google/Chrome Beta",
    "Google/Chrome Canary",
    "Chromium",
    "BraveSoftware/Brave-Browser",
    "Microsoft Edge",
    "Vivaldi",
    "com.operasoftware.Opera",
    "Arc",
    "Dia/User Data",
]


def cookie_dbs(home: Path) -> list[Path]:
    support = home / "Library" / "Application Support"
    found: set[Path] = set()
    for app in CHROMIUM_ROOTS:
        root = support / app
        for pattern in ("*/Cookies", "*/Network/Cookies"):
            found.update(root.glob(pattern))
    return sorted(found)


def clear_sqlite(path: str, bases: list[str]) -> tuple[int, str]:
    try:
        con = sqlite3.connect(f"file:{path}?mode=rw", uri=True, timeout=1.0)
    except sqlite3.OperationalError as exc:
        return 0, f"error: {exc}"
    try:
        cur = con.cursor()
        cur.execute("SELECT DISTINCT host_key FROM cookies")
        hosts = [row[0] for row in cur.fetchall() if matches(row[0], bases)]
        for host in hosts:
            cur.execute("DELETE FROM cookies WHERE host_key = ?", (host,))
        con.commit()
        return len(hosts), "ok"
    except sqlite3.OperationalError as exc:
        if "locked" in str(exc):
            # Held EXCLUSIVE by a running browser without a debug port. The site
            # is DNS-blocked so it is unreachable meanwhile; it gets purged on
            # the next switch after the browser closes.
            return 0, "locked (browser running)"
        return 0, f"error: {exc}"
    except Exception as exc:
        return 0, f"error: {exc}"
    finally:
        con.close()


# ---------------------------------------------------------------------------
# Running browsers: Chrome DevTools Protocol over a minimal websocket.
# ---------------------------------------------------------------------------


def _ws_connect(host: str, port: int, path: str) -> socket.socket:
    sock = socket.create_connection((host, port), timeout=2.0)
    key = base64.b64encode(os.urandom(16)).decode()
    handshake = (
        f"GET {path} HTTP/1.1\r\n"
        f"Host: {host}:{port}\r\n"
        "Upgrade: websocket\r\n"
        "Connection: Upgrade\r\n"
        f"Sec-WebSocket-Key: {key}\r\n"
        "Sec-WebSocket-Version: 13\r\n\r\n"
    )
    sock.sendall(handshake.encode())
    buf = b""
    while b"\r\n\r\n" not in buf:
        chunk = sock.recv(4096)
        if not chunk:
            raise ConnectionError("websocket handshake failed")
        buf += chunk
    status_line = buf.split(b"\r\n", 1)[0]
    if b" 101 " not in status_line:
        # A redirect, 4xx, or non-CDP server squatting on the port: bail before
        # framing websocket bytes onto a plain-HTTP socket.
        raise ConnectionError(f"websocket upgrade refused: {status_line.decode(errors='replace')}")
    return sock


def _ws_send(sock: socket.socket, text: str) -> None:
    payload = text.encode()
    header = bytearray([0x81])  # FIN + text opcode
    length = len(payload)
    mask = os.urandom(4)
    if length < 126:
        header.append(0x80 | length)
    elif length < 65536:
        header.append(0x80 | 126)
        header += struct.pack(">H", length)
    else:
        header.append(0x80 | 127)
        header += struct.pack(">Q", length)
    header += mask
    masked = bytes(byte ^ mask[i % 4] for i, byte in enumerate(payload))
    sock.sendall(bytes(header) + masked)


def _ws_recv(sock: socket.socket) -> str:
    def read_exactly(count: int) -> bytes:
        data = b""
        while len(data) < count:
            chunk = sock.recv(count - len(data))
            if not chunk:
                raise ConnectionError("websocket closed")
            data += chunk
        return data

    out = b""
    while True:
        head = read_exactly(2)
        fin = head[0] & 0x80
        opcode = head[0] & 0x0F
        length = head[1] & 0x7F
        if length == 126:
            length = struct.unpack(">H", read_exactly(2))[0]
        elif length == 127:
            length = struct.unpack(">Q", read_exactly(8))[0]
        data = read_exactly(length)
        if opcode == 0x8:
            raise ConnectionError("websocket closed by peer")
        if opcode in (0x9, 0xA):  # ping/pong control frame: skip
            continue
        out += data
        if fin:
            return out.decode()


class Cdp:
    def __init__(self, sock: socket.socket) -> None:
        self.sock = sock
        self.next_id = 0

    def call(self, method: str, params: dict | None = None) -> dict:
        self.next_id += 1
        message_id = self.next_id
        _ws_send(self.sock, json.dumps({"id": message_id, "method": method, "params": params or {}}))
        while True:
            message = json.loads(_ws_recv(self.sock))
            if message.get("id") == message_id:
                if "error" in message:
                    raise RuntimeError(message["error"])
                return message.get("result", {})


def _http_json(url: str) -> object | None:
    if parse.urlsplit(url).scheme not in {"http", "https"}:
        raise ValueError("CDP discovery URL must use HTTP")
    try:
        with request.urlopen(url, timeout=1.0) as resp:  # noqa: S310
            return json.load(resp)
    except (error.URLError, OSError, ValueError):
        return None


def clear_cdp(port: int, bases: list[str]) -> tuple[int, str] | None:
    targets = _http_json(f"http://127.0.0.1:{port}/json/list")
    if not isinstance(targets, list):
        return None  # nothing speaking CDP on this port
    try:
        pages = [t for t in targets if t.get("type") == "page" and t.get("webSocketDebuggerUrl")]
        if not pages:
            return 0, "no page target"
        ws_path = parse.urlsplit(pages[0]["webSocketDebuggerUrl"]).path
        sock = _ws_connect("127.0.0.1", port, ws_path)
        try:
            cdp = Cdp(sock)
            cdp.call("Network.enable")
            # Default browser context only: incognito and CHIPS-partitioned
            # cookies (those with a partitionKey) are not covered. Fine for the
            # "logged out of a blocked site" goal.
            cookies = cdp.call("Network.getAllCookies").get("cookies", [])
            hits = [c for c in cookies if matches(c["domain"], bases)]
            for cookie in hits:
                cdp.call(
                    "Network.deleteCookies",
                    {"name": cookie["name"], "domain": cookie["domain"], "path": cookie.get("path", "/")},
                )
            return len(hits), "ok (cdp)"
        finally:
            sock.close()
    except Exception as exc:
        # Best-effort: a dropped socket or odd CDP reply must not abort the
        # home-manager switch this runs inside.
        return 0, f"error: {exc}"


# Dia uses 9223; Chrome's default is 9222 when launched with the flag.
DEBUG_PORTS = (9222, 9223, 9224)


def main() -> int:
    bases = [arg.strip().lower().lstrip(".") for arg in sys.argv[1:] if arg.strip()]
    if not bases:
        print("clear-blocked-cookies: no blocked domains; nothing to do")
        return 0

    report: list[tuple[str, int, str]] = []

    for port in DEBUG_PORTS:
        result = clear_cdp(port, bases)
        if result is not None:
            count, status = result
            report.append((f"cdp:{port}", count, status))

    home = Path("~").expanduser()
    for db_path in cookie_dbs(home):
        count, status = clear_sqlite(db_path, bases)
        report.append((str(db_path.relative_to(home)), count, status))

    total = sum(count for _, count, _ in report)
    for name, count, status in report:
        if count or status not in ("ok", "no page target"):
            print(f"  {name}: {count} cleared [{status}]")
    print(f"clear-blocked-cookies: removed {total} cookie(s) for {len(bases)} blocked domain(s)")
    return 0


if __name__ == "__main__":
    # This runs inside a home-manager activation under `set -euo pipefail`, so a
    # non-zero exit aborts the whole switch. Cookie clearing is best-effort:
    # never let it break an otherwise-good switch.
    try:
        raise SystemExit(main())
    except SystemExit:
        raise
    except Exception as exc:
        print(f"clear-blocked-cookies: non-fatal error: {exc}", file=sys.stderr)
        raise SystemExit(0) from exc
