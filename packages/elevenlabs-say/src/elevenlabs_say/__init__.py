"""A say-style ElevenLabs text-to-speech CLI.

Reads text from a positional argument, a file, or stdin, synthesizes speech with
the ElevenLabs API, and either plays it through the speakers with ``ffplay`` or
writes the audio to a file. The API key comes from ``ELEVENLABS_API_KEY``; there
is no embedded key and no silent fallback.

With ``--stream`` the CLI uses the ElevenLabs WebSocket input-streaming endpoint:
stdin is forwarded token by token and audio is played as it arrives, so it can
sit at the end of a pipe that emits text incrementally (for example an LLM token
stream) instead of waiting for the whole input.
"""

from __future__ import annotations

import argparse
import codecs
import os
import subprocess
import sys
import tempfile
from collections.abc import Iterable, Iterator
from dataclasses import dataclass
from pathlib import Path

from elevenlabs import ElevenLabs
from elevenlabs.core import ApiError
from elevenlabs.realtime_tts import RealtimeTextToSpeechClient

# George is a current ElevenLabs default voice and the one the official quickstart
# ships, so it works on every tier including free. The legacy premade voices (such
# as Rachel, 21m00Tcm4TlvDq8ikWAM) became Voice Library voices, which the API
# refuses for free accounts with HTTP 402 paid_plan_required. Default voices are
# scheduled to retire on 2026-12-31, so this id may need refreshing then.
# https://elevenlabs.io/docs/quickstart
DEFAULT_VOICE_ID = "JBFqnCBsd6RMkjVDRZzb"
DEFAULT_MODEL_ID = "eleven_flash_v2_5"
DEFAULT_OUTPUT_FORMAT = "mp3_44100_128"

API_KEY_ENV = "ELEVENLABS_API_KEY"

# Shared so the batch and streaming playback paths report the same fix.
FFPLAY_NOT_FOUND = (
    "ffplay was not found on PATH; install ffmpeg to play audio, "
    "or use --output PATH to save the audio instead"
)


class SayError(Exception):
    """An operator-facing failure with an actionable message."""


@dataclass(frozen=True)
class CliArgs:
    text: str | None
    file: Path | None
    output: Path | None
    voice: str
    model: str
    output_format: str
    # None means auto: stream when text is piped on stdin, batch otherwise.
    stream: bool | None


def parse_args(argv: list[str] | None = None) -> CliArgs:
    parser = argparse.ArgumentParser(
        prog="elevenlabs-say",
        description="Synthesize speech with ElevenLabs and play it or save it to a file.",
    )
    _ = parser.add_argument(
        "text",
        nargs="?",
        default=None,
        help="Text to speak. Omit to read from --file or stdin.",
    )
    _ = parser.add_argument(
        "-f",
        "--file",
        type=Path,
        default=None,
        help="Read text from this file instead of the positional argument.",
    )
    _ = parser.add_argument(
        "-o",
        "--output",
        type=Path,
        default=None,
        help="Write audio to this file instead of playing it.",
    )
    _ = parser.add_argument(
        "--voice",
        default=DEFAULT_VOICE_ID,
        help=(
            "Voice name or id. A value that matches a voice name is resolved to "
            f"its id; otherwise it is used verbatim. Defaults to George ({DEFAULT_VOICE_ID})."
        ),
    )
    _ = parser.add_argument(
        "--model",
        default=DEFAULT_MODEL_ID,
        help=f"Model id. Defaults to {DEFAULT_MODEL_ID}.",
    )
    _ = parser.add_argument(
        "--format",
        dest="output_format",
        default=DEFAULT_OUTPUT_FORMAT,
        help=f"Output audio format. Defaults to {DEFAULT_OUTPUT_FORMAT}.",
    )
    _ = parser.add_argument(
        "--stream",
        action=argparse.BooleanOptionalAction,
        default=None,
        help=(
            "Stream input over the ElevenLabs WebSocket: forward stdin as it "
            "arrives and play or write audio incrementally. Defaults on when text "
            "is piped on stdin and off for TEXT or --file; pass --stream or "
            "--no-stream to force it."
        ),
    )
    namespace = parser.parse_args(argv)

    text: str | None = namespace.text
    file: Path | None = namespace.file
    output: Path | None = namespace.output
    voice: str = namespace.voice
    model: str = namespace.model
    output_format: str = namespace.output_format
    stream: bool | None = namespace.stream

    return CliArgs(
        text=text,
        file=file,
        output=output,
        voice=voice,
        model=model,
        output_format=output_format,
        stream=stream,
    )


def read_text(args: CliArgs) -> str:
    """Resolve the text to speak: positional arg, then --file, then stdin."""
    if args.text is not None:
        source = args.text
    elif args.file is not None:
        try:
            source = args.file.read_text(encoding="utf-8")
        except OSError as exc:
            raise SayError(f"cannot read text file {args.file}: {exc}") from exc
    elif not sys.stdin.isatty():
        source = sys.stdin.read()
    else:
        raise SayError(
            "no text to speak: pass TEXT, use --file PATH, or pipe text on stdin"
        )

    text = source.strip()
    if not text:
        raise SayError("no text to speak: the resolved text is empty")
    return text


def stdin_text_chunks() -> Iterator[str]:
    """Yield decoded text from stdin as each read returns.

    ``os.read`` returns as soon as any bytes are available, so a slow producer's
    tokens are forwarded immediately. Iterating ``sys.stdin`` line by line would
    instead block until a newline, which a token stream may never emit. An
    incremental UTF-8 decoder keeps multi-byte characters intact when one is split
    across two reads.
    """
    decoder = codecs.getincrementaldecoder("utf-8")()
    fd = sys.stdin.fileno()
    while True:
        data = os.read(fd, 4096)
        if not data:
            tail = decoder.decode(b"", final=True)
            if tail:
                yield tail
            return
        text = decoder.decode(data)
        if text:
            yield text


def text_source(args: CliArgs) -> Iterator[str]:
    """Yield text incrementally for streaming synthesis.

    A positional argument or --file resolves to a single chunk; stdin is read in
    small reads so a producer that streams tokens is forwarded with low latency
    rather than buffered until EOF.
    """
    if args.text is not None:
        yield args.text
    elif args.file is not None:
        try:
            yield args.file.read_text(encoding="utf-8")
        except OSError as exc:
            raise SayError(f"cannot read text file {args.file}: {exc}") from exc
    elif not sys.stdin.isatty():
        yield from stdin_text_chunks()
    else:
        raise SayError(
            "no text to speak: pass TEXT, use --file PATH, or pipe text on stdin"
        )


def make_client() -> ElevenLabs:
    if not os.environ.get(API_KEY_ENV):
        raise SayError(
            f"{API_KEY_ENV} is not set; export your ElevenLabs API key, "
            f"for example: export {API_KEY_ENV}=sk_..."
        )
    return ElevenLabs()


def stream_client(client: ElevenLabs) -> RealtimeTextToSpeechClient:
    """Narrow ``client.text_to_speech`` to the realtime client.

    ``ElevenLabs.__init__`` wires ``text_to_speech`` to a
    ``RealtimeTextToSpeechClient``, but the inherited accessor is typed as the
    base ``TextToSpeechClient``, so ``convert_realtime`` is invisible to the type
    checker. Narrow it here and fail loudly if a future SDK drops the wiring.
    """
    tts = client.text_to_speech
    if not isinstance(tts, RealtimeTextToSpeechClient):
        raise SayError(
            "this elevenlabs build does not expose WebSocket input streaming "
            "(RealtimeTextToSpeechClient); --stream needs elevenlabs>=2.0"
        )
    return tts


def resolve_voice_id(client: ElevenLabs, voice: str) -> str:
    """Treat ``voice`` as a name first; fall back to using it as an id verbatim.

    ElevenLabs voice ids are opaque 20-character tokens, so a human-typed name
    almost never collides with an id. Searching by name keeps the CLI usable with
    friendly voice names while still accepting a raw id.
    """
    try:
        response = client.voices.search(search=voice)
    except ApiError as exc:
        raise SayError(format_api_error("resolve voice", exc)) from exc

    for candidate in response.voices:
        if candidate.name is not None and candidate.name.casefold() == voice.casefold():
            return candidate.voice_id

    # No name match: use the supplied value as a literal voice id.
    return voice


def synthesize(client: ElevenLabs, text: str, args: CliArgs, voice_id: str) -> bytes:
    try:
        chunks = client.text_to_speech.convert(
            voice_id=voice_id,
            text=text,
            model_id=args.model,
            output_format=args.output_format,
        )
        return b"".join(chunks)
    except ApiError as exc:
        raise SayError(format_api_error("synthesize speech", exc)) from exc


def stream_synthesize(
    client: ElevenLabs, args: CliArgs, voice_id: str
) -> Iterator[bytes]:
    """Stream audio for stdin text over the WebSocket input-streaming endpoint.

    The SDK generator interleaves sending each text chunk with a short
    non-blocking receive, so audio comes back while later input is still being
    typed. It pins ``chunk_length_schedule=[50]`` for low latency, which is the
    intended trade for a pipe.
    """
    # voice_settings must be passed explicitly as None. The SDK defaults it to its
    # OMIT sentinel (the Ellipsis ...), then builds the init frame with
    # `voice_settings.dict() if voice_settings else None`; Ellipsis is truthy, so
    # the default crashes with AttributeError. None is falsy and means "use the
    # voice's stored settings". https://github.com/elevenlabs/elevenlabs-python
    return stream_client(client).convert_realtime(
        voice_id=voice_id,
        text=text_source(args),
        model_id=args.model,
        output_format=args.output_format,
        voice_settings=None,
    )


def format_api_error(action: str, exc: ApiError) -> str:
    if exc.status_code is not None:
        return f"failed to {action}: ElevenLabs API returned status {exc.status_code}: {exc.body}"
    return f"failed to {action}: {exc.body}"


def write_output(audio: bytes, output: Path) -> None:
    try:
        _ = output.write_bytes(audio)
    except OSError as exc:
        raise SayError(f"cannot write audio to {output}: {exc}") from exc


def write_stream(chunks: Iterable[bytes], output: Path) -> None:
    """Write audio chunks to ``output`` as they arrive, failing on empty input."""
    wrote = False
    try:
        with output.open("wb") as handle:
            for chunk in chunks:
                _ = handle.write(chunk)
                wrote = True
    except OSError as exc:
        raise SayError(f"cannot write audio to {output}: {exc}") from exc

    if not wrote:
        # An empty mp3 is worse than no file: it looks like success.
        output.unlink(missing_ok=True)
        raise SayError("no audio was produced; the input may have been empty")


def play(audio: bytes) -> None:
    """Play MP3 bytes through the speakers with ``ffplay``.

    ``ffplay`` is provided by ``ffmpeg``, which the Nix wrapper puts on PATH. It
    is the cross-platform, Nix-pinnable counterpart to macOS ``afplay``.
    """
    with tempfile.NamedTemporaryFile(suffix=".mp3", delete=False) as handle:
        temp_path = Path(handle.name)
        _ = handle.write(audio)
    try:
        completed = subprocess.run(
            [
                "ffplay",
                "-nodisp",
                "-autoexit",
                "-loglevel",
                "error",
                str(temp_path),
            ],
            check=False,
        )
        if completed.returncode != 0:
            raise SayError(f"ffplay exited with status {completed.returncode}")
    except FileNotFoundError as exc:
        raise SayError(FFPLAY_NOT_FOUND) from exc
    finally:
        temp_path.unlink(missing_ok=True)


def play_stream(chunks: Iterable[bytes]) -> None:
    """Pipe MP3 audio chunks to ``ffplay`` as they arrive for low-latency playback.

    ``ffplay`` reads from ``pipe:0`` and starts decoding before EOF, so audio
    begins while later chunks are still synthesizing. The process starts lazily
    on the first chunk, so empty input gives a clear error instead of an ffplay
    "no stream found" failure.
    """
    proc: subprocess.Popen[bytes] | None = None
    try:
        for chunk in chunks:
            if proc is None:
                proc = _spawn_ffplay()
            stdin = proc.stdin
            assert stdin is not None  # PIPE is always set; narrows for the checker
            try:
                _ = stdin.write(chunk)
                stdin.flush()
            except BrokenPipeError:
                # ffplay exited early; stop feeding and report via its exit code.
                break
    finally:
        if proc is not None and proc.stdin is not None:
            proc.stdin.close()

    if proc is None:
        raise SayError("no audio was produced; the input may have been empty")

    returncode = proc.wait()
    if returncode != 0:
        raise SayError(f"ffplay exited with status {returncode}")


def _spawn_ffplay() -> subprocess.Popen[bytes]:
    try:
        return subprocess.Popen(
            ["ffplay", "-nodisp", "-autoexit", "-loglevel", "error", "-i", "pipe:0"],
            stdin=subprocess.PIPE,
        )
    except FileNotFoundError as exc:
        raise SayError(FFPLAY_NOT_FOUND) from exc


def should_stream(args: CliArgs) -> bool:
    """Decide whether to take the streaming path.

    An explicit --stream/--no-stream wins. Otherwise stream when text is piped on
    stdin, the case where incremental playback matters, and stay on the batch path
    for a positional argument or --file, where the whole text is already in hand
    and the higher-quality convert endpoint costs nothing extra.
    """
    if args.stream is not None:
        return args.stream
    return args.text is None and args.file is None and not sys.stdin.isatty()


def run(args: CliArgs) -> None:
    client = make_client()

    if should_stream(args):
        voice_id = resolve_voice_id(client, args.voice)
        chunks = stream_synthesize(client, args, voice_id)
        try:
            if args.output is not None:
                write_stream(chunks, args.output)
                print(f"wrote {args.output}", file=sys.stderr)
            else:
                play_stream(chunks)
        except ApiError as exc:
            raise SayError(format_api_error("stream speech", exc)) from exc
        return

    text = read_text(args)
    voice_id = resolve_voice_id(client, args.voice)
    audio = synthesize(client, text, args, voice_id)

    if args.output is not None:
        write_output(audio, args.output)
        print(f"wrote {args.output}", file=sys.stderr)
    else:
        play(audio)


def main() -> None:
    args = parse_args()
    try:
        run(args)
    except SayError as exc:
        print(f"elevenlabs-say: {exc}", file=sys.stderr)
        raise SystemExit(1) from exc
