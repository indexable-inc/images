"""Minecraft server-list ping against localhost:25565.

Speaks the real client protocol (handshake with next_state=status, then a
status request) and asserts the server answers with a well-formed status JSON.
This proves the Minestom network stack is serving actual Minecraft clients,
not merely that something bound the port.

Protocol reference: https://minecraft.wiki/w/Java_Edition_protocol
"""

import json
import socket
import struct
import sys


def varint(n: int) -> bytes:
    out = b""
    while True:
        b = n & 0x7F
        n >>= 7
        out += struct.pack("B", b | (0x80 if n else 0))
        if not n:
            return out


def packet(pid: int, payload: bytes) -> bytes:
    body = varint(pid) + payload
    return varint(len(body)) + body


def read_varint(sock: socket.socket) -> int:
    n = 0
    for i in range(5):
        (b,) = sock.recv(1)
        n |= (b & 0x7F) << (7 * i)
        if not b & 0x80:
            return n
    msg = "varint too long"
    raise ValueError(msg)


def main() -> None:
    host, port = "127.0.0.1", 25565
    with socket.create_connection((host, port), timeout=30) as sock:
        addr = host.encode()
        # Protocol 775 = Minecraft 26.1.2, matching the pinned Minestom build;
        # the status handshake accepts any value, so a bump never breaks this.
        handshake = (
            varint(775) + varint(len(addr)) + addr + struct.pack(">H", port) + varint(1)
        )
        sock.sendall(packet(0, handshake) + packet(0, b""))
        read_varint(sock)  # frame length
        assert read_varint(sock) == 0, "unexpected packet id"
        length = read_varint(sock)
        data = b""
        while len(data) < length:
            chunk = sock.recv(length - len(data))
            assert chunk, "connection closed early"
            data += chunk
    status = json.loads(data.decode())
    print(json.dumps(status, indent=2)[:500])
    assert "version" in status, status
    assert "players" in status, status
    print("PING OK")


if __name__ == "__main__":
    sys.exit(main())
