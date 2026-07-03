#!/usr/bin/env python3
"""Synthetic panes guest for host-side latency measurement (index#1686).

Listens on a unix socket, speaks the panes wire protocol (postcard frames,
see panes-protocol), maps one toplevel, and streams ack-paced damage while
recording the send-to-ack round trip of every frame. Point a host at it:

    python3 tools/latency_probe.py /tmp/panes-probe.sock --render-ms 4 &
    PANES_TRACE=1 panes-host --connect /tmp/panes-probe.sock

Knobs map to the pipeline under test:
  --render-ms  sleep between receiving an ack and sending the next frame,
               simulating guest render time (the compositor's dmabuf
               readback + encode sits in the same position).
  --credit     frames allowed in flight before waiting for an ack. 1 is the
               compositor's pacing today; 2 measures the pipelined regime
               the host's cumulative acks already permit.
  --width/--height/--scale
               buffer size (payload volume) and the advertised buffer scale.

Every frame damages the full buffer with incompressible bands (worst-case
transport), LZ4-encoded when the `lz4` module is importable, Raw otherwise
(both always legal on the wire). The host side of each stage comes from its
`PANES_TRACE=1` stderr lines; timestamps there share the clock of
`time.monotonic()` here, so the two logs correlate directly.

Stdlib-only on purpose (optional lz4), same as gen_keymap.py.
"""

import argparse
import os
import queue
import random
import socket
import struct
import threading
import time
from dataclasses import dataclass, field
from pathlib import Path

try:
    import lz4.block as _lz4
except ImportError:
    _lz4 = None


def _compress(raw: bytes) -> bytes | None:
    """LZ4 block (the wire's Lz4 encoding), or None when lz4 is absent."""
    if _lz4 is None:
        return None
    return _lz4.compress(raw, store_size=False)


# ToHost variant indices (postcard encodes the declaration order).
TOHOST_HELLO = 0
TOHOST_WINDOW_NEW = 1
TOHOST_WINDOW_FRAME = 4
TOHOST_WINDOW_GONE = 5
TOHOST_PONG = 7

# ToGuest variant indices.
TOGUEST_HELLO = 0
TOGUEST_ACK = 1
TOGUEST_CONFIGURE = 2
TOGUEST_CLOSE = 3
TOGUEST_PING = 9

WINDOW_ID = 1
BAND_ROWS = 256


def uvarint(value: int) -> bytes:
    out = bytearray()
    while True:
        byte = value & 0x7F
        value >>= 7
        if value:
            out.append(byte | 0x80)
        else:
            out.append(byte)
            return bytes(out)


def read_uvarint(buf: bytes, pos: int) -> tuple[int, int]:
    shift = 0
    value = 0
    while True:
        byte = buf[pos]
        pos += 1
        value |= (byte & 0x7F) << shift
        if not byte & 0x80:
            return value, pos
        shift += 7


def wire_frame(payload: bytes) -> bytes:
    """`[u32 LE len][postcard bytes]`, the panes-protocol framing."""
    return struct.pack("<I", len(payload)) + payload


def msg_hello() -> bytes:
    return uvarint(TOHOST_HELLO) + uvarint(1) + uvarint(1)


def msg_window_new(width: int, height: int, scale: int) -> bytes:
    title = b"latency probe"
    app_id = b"dev.ix.panes.probe"
    return (
        uvarint(TOHOST_WINDOW_NEW)
        + uvarint(WINDOW_ID)
        + uvarint(len(title))
        + title
        + uvarint(len(app_id))
        + app_id
        + uvarint(width)
        + uvarint(height)
        + uvarint(scale)
    )


def msg_window_gone() -> bytes:
    return uvarint(TOHOST_WINDOW_GONE) + uvarint(WINDOW_ID)


def msg_pong(nonce: int) -> bytes:
    return uvarint(TOHOST_PONG) + uvarint(nonce)


@dataclass
class Band:
    y: int
    height: int
    encoding: int  # 0 = Raw, 1 = Lz4
    payload: bytes


def make_bands(width: int, height: int, variant: int) -> list[Band]:
    """Full-height damage in incompressible BAND_ROWS bands."""
    rng = random.Random(variant)
    bands = []
    y = 0
    while y < height:
        band_h = min(BAND_ROWS, height - y)
        raw_len = width * band_h * 4
        # Half noise, half a repeated chunk: compresses ~2x like real content.
        chunk = bytes(rng.randrange(256) for _ in range(4096))
        noise = os.urandom(raw_len // 2)
        raw = noise + (chunk * (raw_len // 4096 + 1))[: raw_len - len(noise)]
        compressed = _compress(raw)
        if compressed is None:
            bands.append(Band(y, band_h, 0, raw))
        else:
            bands.append(Band(y, band_h, 1, compressed))
        y += band_h
    return bands


def msg_window_frame(
    seq: int, width: int, height: int, *, full: bool, bands: list[Band]
) -> bytes:
    out = bytearray(
        uvarint(TOHOST_WINDOW_FRAME)
        + uvarint(WINDOW_ID)
        + uvarint(seq)
        + uvarint(width)
        + uvarint(height)
        + (b"\x01" if full else b"\x00")
        + uvarint(len(bands))
    )
    for band in bands:
        out += uvarint(0) + uvarint(band.y) + uvarint(width) + uvarint(band.height)
        out += uvarint(band.encoding) + uvarint(len(band.payload)) + band.payload
    return bytes(out)


@dataclass
class Event:
    kind: str
    seq: int = 0
    nonce: int = 0


@dataclass
class Stats:
    send_at: dict[int, float] = field(default_factory=dict)
    presented_at: dict[int, float] = field(default_factory=dict)
    coalesced: set[int] = field(default_factory=set)

    def report(self, skip: int = 20) -> str:
        """Acks are cumulative ("presented up to seq"), so with credit > 1 an
        ack can cover older frames whose present was skipped. Only the
        exactly acked seq was presented: those alone feed the RTT
        distribution and presented_fps; covered-but-skipped frames count as
        `coalesced` (the guest-side produce rate, not a display rate)."""
        pairs = sorted(
            (seq, self.send_at[seq], at)
            for seq, at in self.presented_at.items()
            if seq in self.send_at
        )[skip:]
        if len(pairs) < 2:
            return "too few acked frames"
        rtts = sorted((ack - sent) * 1000 for _, sent, ack in pairs)
        span = pairs[-1][2] - pairs[0][2]
        presented_fps = (len(pairs) - 1) / span if span > 0 else 0.0
        produced = len(pairs) + len(self.coalesced)
        produced_fps = (produced - 1) / span if span > 0 else 0.0

        def pick(quantile: float) -> float:
            return rtts[min(len(rtts) - 1, int(len(rtts) * quantile))]

        return (
            f"n={len(rtts)} presented_fps={presented_fps:.1f} "
            f"produced_fps~={produced_fps:.1f} coalesced={len(self.coalesced)} "
            f"present_rtt_ms p50={pick(0.5):.2f} p90={pick(0.9):.2f} "
            f"p99={pick(0.99):.2f} max={rtts[-1]:.2f}"
        )


def decode_toguest(buf: bytes) -> Event:
    disc, pos = read_uvarint(buf, 0)
    if disc == TOGUEST_HELLO:
        return Event("hello")
    if disc == TOGUEST_ACK:
        _, pos = read_uvarint(buf, pos)
        seq, _ = read_uvarint(buf, pos)
        return Event("ack", seq=seq)
    if disc == TOGUEST_CLOSE:
        return Event("close")
    if disc == TOGUEST_PING:
        nonce, _ = read_uvarint(buf, pos)
        return Event("ping", nonce=nonce)
    return Event("other")


def read_loop(conn: socket.socket, events: "queue.Queue[Event]") -> None:
    stream = conn.makefile("rb")
    while True:
        header = stream.read(4)
        if len(header) < 4:
            events.put(Event("eof"))
            return
        (length,) = struct.unpack("<I", header)
        events.put(decode_toguest(stream.read(length)))


def drive(conn: socket.socket, args: argparse.Namespace) -> Stats:
    events: queue.Queue[Event] = queue.Queue()
    threading.Thread(target=read_loop, args=(conn, events), daemon=True).start()
    conn.sendall(wire_frame(msg_hello()))
    while events.get().kind != "hello":
        pass
    conn.sendall(wire_frame(msg_window_new(args.width, args.height, args.scale)))

    variants = [make_bands(args.width, args.height, v) for v in range(4)]
    stats = Stats()
    seq = 0
    deadline = time.monotonic() + args.seconds

    def send_next(*, full: bool) -> None:
        nonlocal seq
        seq += 1
        frame = msg_window_frame(
            seq, args.width, args.height, full=full, bands=variants[seq % len(variants)]
        )
        stats.send_at[seq] = time.monotonic()
        conn.sendall(wire_frame(frame))

    for _ in range(args.credit):
        send_next(full=seq == 0)
    inflight = seq
    while time.monotonic() < deadline:
        try:
            event = events.get(timeout=2.0)
        except queue.Empty:
            continue
        if event.kind == "eof":
            break
        if event.kind == "ping":
            conn.sendall(wire_frame(msg_pong(event.nonce)))
        elif event.kind == "close":
            conn.sendall(wire_frame(msg_window_gone()))
            break
        elif event.kind == "ack":
            now = time.monotonic()
            # Exactly acked = presented; older seqs the cumulative ack covers
            # were coalesced away, never presented (see Stats.report).
            if event.seq in stats.send_at:
                stats.presented_at.setdefault(event.seq, now)
            for pending in stats.send_at:
                if pending < event.seq and pending not in stats.presented_at:
                    stats.coalesced.add(pending)
            inflight = seq - event.seq
            while inflight < args.credit:
                if args.render_ms:
                    time.sleep(args.render_ms / 1000)
                send_next(full=False)
                inflight += 1
    return stats


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("socket_path")
    parser.add_argument("--width", type=int, default=1728)
    parser.add_argument("--height", type=int, default=1080)
    parser.add_argument("--scale", type=int, default=2)
    parser.add_argument("--credit", type=int, default=1)
    parser.add_argument("--render-ms", type=float, default=0.0)
    parser.add_argument("--seconds", type=float, default=10.0)
    args = parser.parse_args()

    # A stale socket file from a previous run would fail the bind.
    Path(args.socket_path).unlink(missing_ok=True)
    listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    listener.bind(args.socket_path)
    listener.listen(1)
    print(f"probe: listening on {args.socket_path}", flush=True)
    conn, _ = listener.accept()
    print("probe: host connected", flush=True)
    stats = drive(conn, args)
    print(f"probe: {stats.report()}", flush=True)


if __name__ == "__main__":
    main()
