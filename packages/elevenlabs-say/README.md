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

# Pipe text on stdin. This streams by default: a producer that emits text over
# time is spoken as it arrives, instead of waiting for it to finish.
my-llm --prompt "tell me a story" | nix run .#elevenlabs-say

# Force the batch path on a pipe (buffer all stdin, then synthesize once).
cat notes.txt | nix run .#elevenlabs-say -- --no-stream

# Save audio instead of playing it.
nix run .#elevenlabs-say -- "save me" --output /tmp/out.mp3

# Pick a voice by name or id, and override the model or format.
nix run .#elevenlabs-say -- "different voice" --voice Adam
nix run .#elevenlabs-say -- "slower model" --model eleven_multilingual_v2 --format mp3_44100_192
```

Text source precedence is positional argument, then `--file`, then stdin.

## Streaming input

When text is piped on stdin, the CLI streams by default: it reads stdin as each
read returns (rather than buffering to EOF), forwards the text token by token over
the ElevenLabs [WebSocket input-streaming
endpoint](https://elevenlabs.io/docs/api-reference/text-to-speech/v-1-text-to-speech-voice-id-stream-input),
and plays or writes audio as it comes back. This is what you want at the end of a
pipe whose producer emits text over time, such as an LLM token stream.

A positional `TEXT` argument or `--file` stays on the batch path, since the whole
text is already in hand and the plain `convert` endpoint gives slightly better
prosody. Override either way with `--stream` or `--no-stream`. Use `--no-stream` on
a pipe when you are feeding a complete document and prefer the batch quality.

Streaming works with `--output` too, writing audio to the file as it arrives. It
honors `--voice`, `--model`, and `--format`; the default `eleven_flash_v2_5` model
is the low-latency choice and supports this endpoint. The model `eleven_v3` does
not, so forcing `--stream` with it will fail.

## Defaults

- Voice: George (`JBFqnCBsd6RMkjVDRZzb`), a current ElevenLabs default voice that
  works on every tier including free. A `--voice` value that matches a voice name
  is resolved to its id; otherwise it is used as a literal id. The legacy premade
  voices (such as Rachel) are now Voice Library voices, which the API refuses for
  free accounts with `402 paid_plan_required`.
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
- `--stream` plays MP3 over a pipe, so it assumes an MP3 `--format` (the
  default). A raw PCM format has no container for `ffplay` to detect from the
  stream and will not play; use the default or `--output` for raw formats.
