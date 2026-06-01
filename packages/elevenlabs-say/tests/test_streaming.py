"""Offline regression tests for the --stream input path.

Run by the ``streaming`` passthru test with the built venv interpreter. Network
is never touched: the WebSocket client is only constructed, not connected.
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
import threading
import time
from pathlib import Path

import elevenlabs.realtime_tts as realtime_module
from elevenlabs.core import ApiError

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


def test_stream_init_does_not_crash_on_omit_voice_settings() -> None:
    """Regression: building convert_realtime's init frame must not trip the SDK's
    OMIT-sentinel bug, where voice_settings defaults to Ellipsis (truthy, no
    .dict()) and crashes. stream_synthesize passes voice_settings=None; assert the
    init frame is sent and carries a null voice_settings.
    """
    os.environ["ELEVENLABS_API_KEY"] = "test-key-not-used"
    client = say.make_client()

    class _StopAfterInit(Exception):
        pass

    class FakeSocket:
        def __init__(self) -> None:
            self.sent: list[str] = []

        def send(self, message: str) -> None:
            self.sent.append(message)
            raise _StopAfterInit

        def recv(self, *args: object) -> str:
            raise _StopAfterInit

    socket = FakeSocket()

    class FakeConnection:
        def __enter__(self) -> FakeSocket:
            return socket

        def __exit__(self, *args: object) -> bool:
            return False

    original_connect = realtime_module.connect
    realtime_module.connect = lambda *a, **k: FakeConnection()  # type: ignore[assignment]
    try:
        chunks = say.stream_synthesize(
            client, say.parse_args(["hi", "--stream"]), "voice-id"
        )
        try:
            next(iter(chunks))
        except _StopAfterInit:
            pass
    finally:
        realtime_module.connect = original_connect  # type: ignore[assignment]

    assert socket.sent, "init frame was never sent"
    init = json.loads(socket.sent[0])
    assert init["voice_settings"] is None, init


def test_atempo_filter_maps_wpm_to_in_range_stages() -> None:
    """--rate maps WPM to an atempo chain whose stages stay in ffmpeg's range and
    multiply back to rate/175. Guards the say -r emulation and the stage decomposition.
    """
    assert say.atempo_filter(None) is None
    for rate in (350, 525, 175, 88, 43, 700, 20):
        chain = say.atempo_filter(rate)
        assert chain is not None
        stages = [float(part.removeprefix("atempo=")) for part in chain.split(",")]
        assert all(0.5 <= s <= 100.0 for s in stages), (rate, stages)
        product = 1.0
        for s in stages:
            product *= s
        assert abs(product - rate / say.DEFAULT_WPM) < 1e-3, (rate, product)
    for bad in (0, -5):
        try:
            say.atempo_filter(bad)
        except say.SayError:
            pass
        else:
            raise AssertionError(f"expected SayError for rate={bad}")


def test_say_compat_flags_parse() -> None:
    """macOS-style `-v` (voice) and `-r` (rate) aliases parse into CliArgs."""
    args = say.parse_args(["hello", "-v", "Adam", "-r", "300"])
    assert args.voice == "Adam"
    assert args.rate == 300
    assert say.parse_args(["hello"]).rate is None


def test_resolve_voice_id_id_shape_skips_api() -> None:
    """A voice-id-shaped value is returned verbatim without touching the API.

    Guards the synthesis-only-key path: resolving the default voice (an id) must
    not call ``voices.search``, which needs the ``voices_read`` permission a
    TTS-only key lacks. Accessing ``.voices`` on the sentinel would raise.
    """

    class Boom:
        def __getattr__(self, name: str) -> object:
            raise AssertionError(f"client.{name} must not be used for an id")

    assert say.resolve_voice_id(Boom(), "JBFqnCBsd6RMkjVDRZzb") == "JBFqnCBsd6RMkjVDRZzb"


def test_resolve_voice_id_name_searches_and_maps() -> None:
    """A friendly name is matched case-insensitively to its id via the search."""

    class FakeVoices:
        def search(self, search: str) -> object:
            assert search == "George"
            voice = type("V", (), {"name": "George", "voice_id": "ID0000000000000000aa"})()
            return type("R", (), {"voices": [voice]})()

    client = type("C", (), {"voices": FakeVoices()})()
    assert say.resolve_voice_id(client, "George") == "ID0000000000000000aa"


def test_resolve_voice_id_missing_permission_is_actionable() -> None:
    """A 401 while resolving a name raises a SayError naming voices_read."""

    class FakeVoices:
        def search(self, search: str) -> object:
            raise ApiError(status_code=401, body={"detail": "nope"})

    client = type("C", (), {"voices": FakeVoices()})()
    try:
        say.resolve_voice_id(client, "George")
    except say.SayError as exc:
        assert "voices_read" in str(exc)
    else:
        raise AssertionError("expected SayError for a 401 during name resolution")


if __name__ == "__main__":
    test_stdin_yields_before_eof_and_rejoins_split_utf8()
    test_write_stream_writes_chunks_and_rejects_empty()
    test_play_stream_rejects_empty_without_spawning_ffplay()
    test_stream_client_narrows_to_realtime()
    test_should_stream_auto_and_overrides()
    test_stream_init_does_not_crash_on_omit_voice_settings()
    test_atempo_filter_maps_wpm_to_in_range_stages()
    test_say_compat_flags_parse()
    test_resolve_voice_id_id_shape_skips_api()
    test_resolve_voice_id_name_searches_and_maps()
    test_resolve_voice_id_missing_permission_is_actionable()
    print("elevenlabs-say streaming tests passed")
