# elevenlabs-say

`packages/elevenlabs-say` is a `say`-style CLI that speaks text with the
[ElevenLabs](https://elevenlabs.io) text-to-speech API. It reads text from a
positional argument, a file, or stdin, synthesizes speech, and either plays it
through the speakers with `ffplay` or writes it to a file (`README.md:1-5`,
`src/elevenlabs_say/__init__.py:1-12`). With stdin it streams by default over the
ElevenLabs WebSocket input-streaming endpoint, so it can sit at the end of a pipe
whose producer emits text over time (an LLM token stream).

Unlike the rest of the domain this is a **Python** app (single module
`src/elevenlabs_say/__init__.py`, `requires-python >= 3.13`), built with
`ix.buildUvApplication`; flake output `.#elevenlabs-say`. Its one runtime
dependency is `elevenlabs>=2.50.0,<3.0.0` (`pyproject.toml:6-8`); the console
script `elevenlabs-say = elevenlabs_say:main` is the entry point
(`pyproject.toml:10-11`).

## Public surface: CLI (`parse_args`, `__init__.py:74-162`)

| flag | macOS `say` analog | default | meaning |
| --- | --- | --- | --- |
| `text` (positional) | | none | text to speak; omit to use `--file` or stdin |
| `-f`, `--file` | `say -f` | none | read text from this file |
| `-o`, `--output` | `say -o` | none | write audio to this file instead of playing |
| `-v`, `--voice` | `say -v` | George (`JBFqnCBsd6RMkjVDRZzb`) | voice name or id |
| `-r`, `--rate WPM` | `say -r` | none | playback tempo in words/min (pitch preserved) |
| `--model` | | `eleven_flash_v2_5` | model id |
| `--format` | | `mp3_44100_128` | output audio format |
| `--stream` / `--no-stream` | | auto | force or disable WebSocket input streaming |

Defaults are constants at `__init__.py:37-42`. Text source precedence is
positional, then `--file`, then stdin (`read_text`/`text_source`,
`__init__.py:165-229`). The API key is read from `ELEVENLABS_API_KEY`
(`API_KEY_ENV`, `__init__.py:44`); `make_client` raises a `SayError` if unset,
with no embedded key and no silent fallback (`__init__.py:232-238`).

## Key flow (`run`, `__init__.py:505-542`)

1. Build the client (env key required) and the optional `atempo` filter from
   `--rate`.
2. `should_stream` (`__init__.py:492-502`): an explicit `--stream`/`--no-stream`
   wins; otherwise stream when text is piped on stdin (where incremental playback
   matters) and batch for a positional arg or `--file` (the whole text is in
   hand and the plain `convert` endpoint gives slightly better prosody).
3. Resolve the voice (`resolve_voice_id`, `__init__.py:264-291`): a value
   matching the 20-char base62 id shape (`_VOICE_ID_RE`, `__init__.py:261`) is
   used verbatim without calling the voices API, so a synthesis-only key lacking
   `voices_read` still works (the common case, including the default voice);
   otherwise it searches voices by name and falls back to treating the value as a
   literal id.
4. Synthesize and play or write.

### Batch path
`synthesize` (`__init__.py:294-304`) calls `client.text_to_speech.convert` and
joins the chunks. With `--output` the bytes are written (after an `atempo` pass
if `--rate` is set); otherwise `play` writes them to a temp `.mp3` and runs
`ffplay -nodisp -autoexit` (`__init__.py:424-445`).

### Streaming path
`stream_synthesize` (`__init__.py:307-328`) narrows the client to a
`RealtimeTextToSpeechClient` (`stream_client`, `__init__.py:241-255`) and calls
`convert_realtime` with `chunk_length_schedule=[50]` for low latency and
`voice_settings=None` (passing `None` explicitly is required: the SDK's `OMIT`
sentinel is the truthy `Ellipsis`, which crashes the init-frame build,
`__init__.py:317-321`). stdin is read with `os.read` in 4096-byte reads through
an incremental UTF-8 decoder so a slow producer's tokens are forwarded
immediately and multi-byte chars split across reads stay intact
(`stdin_text_chunks`, `__init__.py:187-207`). Audio is piped to `ffplay` via
`pipe:0` as it arrives (`play_stream`, `__init__.py:448-478`) or written
incrementally with `--output` (`write_stream`, `__init__.py:407-421`); empty
input is a clear error rather than a silent zero-byte file. The default
`eleven_flash_v2_5` supports this endpoint; `eleven_v3` does not, so forcing
`--stream` with it fails (`README.md:73-75`).

## `--rate` / atempo (`__init__.py:337-397`)

`atempo_filter` builds an ffmpeg `atempo` chain emulating `say -r`: `tempo =
rate / 175` (`DEFAULT_WPM`, `__init__.py:42`). Since `atempo` accepts only
0.5..100.0 per stage, the multiplier is factored into in-range chained stages.
It changes tempo while preserving pitch and does not change synthesis, so it
works on every model including the default Flash (whose API `speed` is ignored).
For playback the filter is passed straight to `ffplay -af`; for `--output` (no
player) `apply_tempo` runs one extra ffmpeg pass (mp3 in, mp3 out), so a raw PCM
`--format` is not supported there (`__init__.py:367-373`, `README.md:51-56`).

## Errors (`SayError`, `__init__.py:56-57`)

All operator-facing failures raise `SayError` with an actionable message;
`main` (`__init__.py:545-551`) prints `elevenlabs-say: <msg>` to stderr and exits
1. API errors are formatted with status + body (`format_api_error`,
`__init__.py:331-334`). A 401/403 while resolving a voice name explains the
`voices_read` permission gap (`__init__.py:277-284`).

## Build and wiring (`default.nix`)

Built with `ix.buildUvApplication` from a `fileset` source of `pyproject.toml`,
`src`, and `uv.lock` (`default.nix:7-31`). `runtimeLibraryInputs =
[ pkgs.stdenv.cc.cc.lib ]` because pydantic-core and websockets ship binary
wheels that dlopen libstdc++ at import on Linux (`default.nix:23-25`). The result
is `makeWrapper`-wrapped to put `pkgs.ffmpeg` (which supplies `ffplay`) on PATH;
`afplay` is macOS-only and absent from nixpkgs, so `ffplay` is the portable
choice (`default.nix:33-50`). Passthru tests: `printsHelp` (asserts `usage:
elevenlabs-say`, `default.nix:52-70`) and `streaming` (exercises the `--stream`
input path offline by constructing the realtime client without connecting,
`default.nix:72-87`, test at `tests/test_streaming.py`).

## Run (`README.md:18-41`)

```
nix run .#elevenlabs-say -- "the first move sets everything in motion"
nix run .#elevenlabs-say -- --file notes.txt
my-llm --prompt "tell me a story" | nix run .#elevenlabs-say     # streams stdin
nix run .#elevenlabs-say -- "save me" --output /tmp/out.mp3
nix run .#elevenlabs-say -- -v Adam -r 300 "talk faster"
```

## Caveats

- Playback needs a working audio device; on a headless host use `--output`
  (`README.md:92-95`).
- `--stream` plays MP3 over a pipe, so it assumes an MP3 `--format`; a raw PCM
  format has no container for `ffplay` to detect from the stream and will not play
  (`README.md:99-101`).
- The default voice id is scheduled to retire 2026-12-31 and may need refreshing
  then (`__init__.py:31-37`).
