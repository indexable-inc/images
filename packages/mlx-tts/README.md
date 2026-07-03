# mlx-tts

Local Apple Silicon text-to-speech through [MLX-Audio](https://github.com/Blaizzy/mlx-audio).
The wrapper defaults to the Chatterbox fp16 model because the request is quality
first, not latency first.

First inference downloads the selected model into the normal Hugging Face cache.
Nix builds only package the CLI and never fetch model weights.

## Usage

```sh
# Quality-first default: Chatterbox.
nix run .#mlx-tts -- "This voice is generated locally on Apple Silicon."

# Voice cloning with a reference clip.
nix run .#mlx-tts -- "Read this in the reference voice." --ref-audio reference.wav --play

# Qwen3 preset when you want preset voices and language control.
nix run .#mlx-tts -- --preset qwen3 --voice Aiden --lang-code English "A preset local voice."

# Pass through any MLX-Audio option after --.
nix run .#mlx-tts -- "More diffusion steps." -- --ddpm_steps 50

# Show the upstream MLX-Audio options.
nix run .#mlx-tts -- --upstream-help
```

Use `--model` to point at any MLX-Audio TTS model repo id or local model path.
