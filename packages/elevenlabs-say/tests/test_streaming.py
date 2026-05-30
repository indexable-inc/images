"""Offline regression tests for the --stream input path.

Run by the ``streaming`` passthru test with the built venv interpreter. Network
is never touched: the WebSocket client is only constructed, not connected.
"""

from __future__ import annotations

import os
import sys
import tempfile
import threading
import time
from pathlib import Path

import elevenlabs_say as say


def test_stdin_yields_before_eof_and_rejoins_split_utf8() -> None:
    """stdin is forwarded as bytes arrive, and a byte-split char is preserved.

    Guards the core streaming contract: a producer that has not closed its end
    still gets its text spoken, and a multi-byte character split across two reads
    is not corrupted. A regression to ``sys.stdin.read()`` or line iteration
    would block until EOF and fail the "before close" assertion.
    """
    read_fd, write_fd = os.pipe()

    class FakeStdin:
        def fileno(self) -> int:
            return read_fd

        def isatty(self) -> bool:
            return False

    original = sys.stdin
    sys.stdin = FakeStdin()  # type: ignore[assignment]
    chunks: list[str] = []

    def drain() -> None:
        for chunk in say.stdin_text_chunks():
            chunks.append(chunk)

    worker = threading.Thread(target=drain)
    worker.start()
    try:
        time.sleep(0.15)
        os.write(write_fd, b"hello ")
        time.sleep(0.15)
        before_close = len(chunks)
        # "ä" is 0xC3 0xA4; split it across two reads.
        os.write(write_fd, b"\xc3")
        time.sleep(0.05)
        os.write(write_fd, b"\xa4 world")
        time.sleep(0.1)
        os.close(write_fd)
        worker.join(timeout=2)
    finally:
        sys.stdin = original

    assert before_close >= 1, f"stdin did not stream before EOF: {chunks}"
    assert "".join(chunks) == "hello ä world", chunks


def test_write_stream_writes_chunks_and_rejects_empty() -> None:
    out = Path(tempfile.mktemp(suffix=".mp3"))
    say.write_stream(iter([b"abc", b"def"]), out)
    assert out.read_bytes() == b"abcdef"
    out.unlink()

    empty = Path(tempfile.mktemp(suffix=".mp3"))
    try:
        say.write_stream(iter([]), empty)
    except say.SayError as exc:
        assert "no audio" in str(exc), exc
    else:
        raise AssertionError("expected SayError on empty stream")
    assert not empty.exists(), "an empty mp3 must not be left behind"


def test_play_stream_rejects_empty_without_spawning_ffplay() -> None:
    try:
        say.play_stream(iter([]))
    except say.SayError as exc:
        assert "no audio" in str(exc), exc
    else:
        raise AssertionError("expected SayError on empty stream")


def test_stream_client_narrows_to_realtime() -> None:
    """The wired client exposes convert_realtime, so --stream has a transport."""
    os.environ["ELEVENLABS_API_KEY"] = "test-key-not-used"
    client = say.make_client()
    realtime = say.stream_client(client)
    assert hasattr(realtime, "convert_realtime"), realtime


def test_should_stream_auto_and_overrides() -> None:
    """Pipe auto-streams, TEXT/--file batch, explicit --stream/--no-stream win."""

    class FakeStdin:
        def __init__(self, tty: bool) -> None:
            self._tty = tty

        def isatty(self) -> bool:
            return self._tty

    original = sys.stdin
    try:
        sys.stdin = FakeStdin(tty=False)  # type: ignore[assignment]
        assert say.should_stream(say.parse_args([])) is True
        assert say.should_stream(say.parse_args(["--no-stream"])) is False
        assert say.should_stream(say.parse_args(["hello"])) is False
        assert say.should_stream(say.parse_args(["hello", "--stream"])) is True
        assert say.should_stream(say.parse_args(["--file", "notes.txt"])) is False

        sys.stdin = FakeStdin(tty=True)  # type: ignore[assignment]
        assert say.should_stream(say.parse_args([])) is False
        assert say.should_stream(say.parse_args(["--stream"])) is True
    finally:
        sys.stdin = original


if __name__ == "__main__":
    test_stdin_yields_before_eof_and_rejoins_split_utf8()
    test_write_stream_writes_chunks_and_rejects_empty()
    test_play_stream_rejects_empty_without_spawning_ffplay()
    test_stream_client_narrows_to_realtime()
    test_should_stream_auto_and_overrides()
    print("elevenlabs-say streaming tests passed")
