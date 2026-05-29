"""A say-style ElevenLabs text-to-speech CLI.

Reads text from a positional argument, a file, or stdin, synthesizes speech with
the ElevenLabs API, and either plays it through the speakers with ``ffplay`` or
writes the audio to a file. The API key comes from ``ELEVENLABS_API_KEY``; there
is no embedded key and no silent fallback.
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path

from elevenlabs import ElevenLabs
from elevenlabs.core import ApiError

# Rachel is a stable ElevenLabs premade voice that is available on every account,
# so it is a safe default for a `say` replacement.
# https://elevenlabs.io/docs/api-reference/voices/get
DEFAULT_VOICE_ID = "21m00Tcm4TlvDq8ikWAM"
DEFAULT_MODEL_ID = "eleven_flash_v2_5"
DEFAULT_OUTPUT_FORMAT = "mp3_44100_128"

API_KEY_ENV = "ELEVENLABS_API_KEY"


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
            f"its id; otherwise it is used verbatim. Defaults to Rachel ({DEFAULT_VOICE_ID})."
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
    namespace = parser.parse_args(argv)

    text: str | None = namespace.text
    file: Path | None = namespace.file
    output: Path | None = namespace.output
    voice: str = namespace.voice
    model: str = namespace.model
    output_format: str = namespace.output_format

    return CliArgs(
        text=text,
        file=file,
        output=output,
        voice=voice,
        model=model,
        output_format=output_format,
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


def make_client() -> ElevenLabs:
    if not os.environ.get(API_KEY_ENV):
        raise SayError(
            f"{API_KEY_ENV} is not set; export your ElevenLabs API key, "
            f"for example: export {API_KEY_ENV}=sk_..."
        )
    return ElevenLabs()


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


def format_api_error(action: str, exc: ApiError) -> str:
    if exc.status_code is not None:
        return f"failed to {action}: ElevenLabs API returned status {exc.status_code}: {exc.body}"
    return f"failed to {action}: {exc.body}"


def write_output(audio: bytes, output: Path) -> None:
    try:
        _ = output.write_bytes(audio)
    except OSError as exc:
        raise SayError(f"cannot write audio to {output}: {exc}") from exc


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
        raise SayError(
            "ffplay was not found on PATH; install ffmpeg to play audio, "
            "or use --output PATH to save the audio instead"
        ) from exc
    finally:
        temp_path.unlink(missing_ok=True)


def run(args: CliArgs) -> None:
    text = read_text(args)
    client = make_client()
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
