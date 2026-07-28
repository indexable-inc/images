from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path

DEFAULT_MODEL = "mlx-community/chatterbox-fp16"
PRESET_MODELS = {
    "chatterbox": DEFAULT_MODEL,
    "qwen3": "mlx-community/Qwen3-TTS-12Hz-1.7B-Base-bf16",
    "kokoro": "mlx-community/Kokoro-82M-bf16",
}


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(
        prog="mlx-tts",
        description="Quality-first local Apple Silicon TTS through MLX-Audio.",
    )
    result.add_argument("text", nargs="?", help="Text to synthesize. Omit to read stdin or prompt interactively.")
    result.add_argument(
        "--preset",
        choices=sorted(PRESET_MODELS),
        default="chatterbox",
        help="Model preset to use when --model is omitted. Default: chatterbox.",
    )
    result.add_argument("--model", help="MLX-Audio model repo id or local path. Overrides --preset.")
    result.add_argument("--voice", help="Model-specific voice name, for example Aiden for Qwen3-TTS.")
    result.add_argument("--ref-audio", action="append", help="Reference audio for voice cloning. May be repeated.")
    result.add_argument("--ref-text", action="append", help="Transcript for a reference audio clip. May be repeated.")
    result.add_argument("--lang-code", default="English", help="Language code for models that need one. Default: English.")
    result.add_argument("--output-path", default=".", help="Directory for generated audio. Default: current directory.")
    result.add_argument("--file-prefix", default="mlx-tts", help="Generated file prefix. Default: mlx-tts.")
    result.add_argument("--audio-format", default="wav", help="Output audio format. Default: wav.")
    result.add_argument("--play", action="store_true", help="Play the generated audio after writing it.")
    result.add_argument(
        "--upstream-help",
        action="store_true",
        help="Show the underlying mlx_audio.tts.generate help.",
    )
    return result


def _extend_repeated(args: list[str], flag: str, values: list[str] | None) -> None:
    if values is None:
        return
    for value in values:
        args.extend([flag, value])


def upstream_args(namespace: argparse.Namespace, extra_args: list[str]) -> list[str]:
    if namespace.upstream_help:
        return ["--help"]

    model = namespace.model or PRESET_MODELS[namespace.preset]
    args = [
        "--model",
        model,
        "--lang_code",
        namespace.lang_code,
        "--output_path",
        str(Path(namespace.output_path)),
        "--file_prefix",
        namespace.file_prefix,
        "--audio_format",
        namespace.audio_format,
    ]
    if namespace.text is not None:
        args.extend(["--text", namespace.text])
    if namespace.voice is not None:
        args.extend(["--voice", namespace.voice])
    _extend_repeated(args, "--ref_audio", namespace.ref_audio)
    _extend_repeated(args, "--ref_text", namespace.ref_text)
    if namespace.play:
        args.append("--play")
    if extra_args:
        args.extend(extra_args[1:] if extra_args[:1] == ["--"] else extra_args)
    return args


def main() -> int:
    namespace, extra_args = parser().parse_known_args()
    return subprocess.call([sys.executable, "-m", "mlx_audio.tts.generate", *upstream_args(namespace, extra_args)])


if __name__ == "__main__":
    raise SystemExit(main())
