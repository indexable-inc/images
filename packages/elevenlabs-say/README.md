# elevenlabs-say

A `say`-style command-line tool that speaks text with the [ElevenLabs](https://elevenlabs.io)
text-to-speech API. It reads text from an argument, a file, or stdin, then plays
the audio through your speakers or writes it to a file.

## Setup

Set your ElevenLabs API key in the environment. The CLI reads it from
`ELEVENLABS_API_KEY` and exits with an error if it is unset.

```sh
export ELEVENLABS_API_KEY=sk_...
```

## Usage

```sh
# Speak a string through the speakers.
nix run .#elevenlabs-say -- "the first move sets everything in motion"

# Speak the contents of a file.
nix run .#elevenlabs-say -- --file notes.txt

# Speak text piped on stdin.
echo "hello from index" | nix run .#elevenlabs-say

# Save audio instead of playing it.
nix run .#elevenlabs-say -- "save me" --output /tmp/out.mp3

# Pick a voice by name or id, and override the model or format.
nix run .#elevenlabs-say -- "different voice" --voice Adam
nix run .#elevenlabs-say -- "slower model" --model eleven_multilingual_v2 --format mp3_44100_192
```

Text source precedence is positional argument, then `--file`, then stdin.

## Defaults

- Voice: Rachel (`21m00Tcm4TlvDq8ikWAM`), a premade voice on every account. A
  `--voice` value that matches a voice name is resolved to its id; otherwise it
  is used as a literal id.
- Model: `eleven_flash_v2_5`, chosen for low latency.
- Format: `mp3_44100_128`.

## Playback

Playback shells out to `ffplay` from `ffmpeg`, which the Nix wrapper puts on
PATH. `--output` skips playback and writes the audio bytes directly.

## Known limitations

- Playback needs a working audio device. On a headless host use `--output` to
  capture the audio instead.
- A name that collides with a 20-character voice id would resolve as a name
  first. ElevenLabs voice ids are opaque tokens, so this does not happen in
  practice.
