---
name: stt
description: Transcribe speech to text locally with whisper.cpp (no API key, runs on hydra's Apple Silicon GPU). Use whenever the user wants to transcribe audio or video, get a transcript, run STT/speech-to-text, caption a file, or pull the words out of a recording (mp4, mov, wav, m4a, mp3, aiff, podcast, voice memo, Twitter/YouTube video). Encodes the validated best-accuracy recipe and the param tradeoffs.
---

# Local Speech-to-Text (whisper.cpp)

Transcribe locally on hydra with no API key. The index repo has **no STT package**
(only `elevenlabs-say` TTS); whisper.cpp from nixpkgs is the tool. See [[whisper-stt-local]].

## The recipe (use this by default)

```bash
# 1. Extract 16 kHz mono WAV (whisper.cpp requires this).
nix shell nixpkgs#ffmpeg -c ffmpeg -y -i INPUT -ar 16000 -ac 1 -c:a pcm_s16le /tmp/aud.wav

# 2. Get the model once (cached in /tmp; ~3 GB).
nix shell nixpkgs#whisper-cpp -c whisper-cpp-download-ggml-model large-v3 /tmp

# 3. Transcribe. --carry-initial-prompt is the load-bearing flag.
nix shell nixpkgs#whisper-cpp -c whisper-cli \
  -m /tmp/ggml-large-v3.bin -f /tmp/aud.wav -nt -otxt -of OUTPUT \
  --prompt "Claude, Claude Code, Anthropic, MCP, <domain names here>" \
  --carry-initial-prompt
```

`-nt` = no timestamps (drop it for timestamped segments; add `-osrt`/`-ovtt` for subtitle files).
Runs ~100 s for a 30 min file on an M-series GPU via Metal.

## Why these choices (measured, not guessed)

Benchmarked on a 30 min podcast. Metric = domain-term hit-rate: every "cloud/quad/clod"
in the audio is really "Claude", so `claude / (claude + mishears)` is a real accuracy proxy.

| config | model | hit-rate | mishears | secs |
|---|---|---|---|---|
| default (no prompt) | large-v3 | 0.16 | 38 | 97 |
| `--prompt` only (no carry) | large-v3 | 0.29 | 32 | 111 |
| **`--prompt` + `--carry-initial-prompt`** | **large-v3** | **0.89** | **5** | 101 |
| + `--vad` | large-v3 | 0.66 | 15 | 110 |
| + `--beam-size 8` | large-v3 | 0.61 | 18 | 115 |
| `--prompt` + carry | large-v2 | 0.80 | 9 | 99 |
| `--prompt` + carry | turbo | 0.36 | 27 | 38 |

**Takeaways:**

1. **`--carry-initial-prompt` is the dominant lever** (0.16 → 0.89, ~7.6x fewer errors).
   A plain `--prompt` only conditions the first ~30 s window; `--carry-initial-prompt`
   re-injects it into *every* window, so domain vocab sticks through a long file. Always
   pass both together. Put the real proper nouns / jargon / names in the prompt.
2. **large-v3 is the most accurate** model. large-v2 is close (0.80) and hallucinates
   slightly less on silence; turbo is much worse on rare words (its decoder is distilled
   to 4 layers) but ~2.6x faster (38 s) — use turbo only when speed beats accuracy and
   the vocab is common.
3. **Skip `--vad` on clean continuous speech** — it *hurt* here (0.89 → 0.66) because
   segment splitting breaks the carried-prompt context. VAD is for noisy audio or long
   silences/music where whisper otherwise hallucinates loops. Reach for it only then:
   add `--vad --vad-model /tmp/ggml-silero-v5.1.2.bin` (download from
   `https://huggingface.co/ggml-org/whisper-vad/resolve/main/ggml-silero-v5.1.2.bin`).
4. **Beam size 5 (default) is enough** — bumping to 8 didn't help. Defaults best-of 5,
   beam 5, temperature fallback are already good; don't fiddle.
5. **Disable previous-context conditioning** with `--max-context 0` only if you see
   repetition/hallucination loops propagating (it stops error carry-over; costs some
   coherence). Not needed on clean audio.

## Cleanup pass

Even at 0.89, a few mishears remain. Sweep them with a word-boundary regex (see
[[whisper-stt-local]] for the pattern); `\bcloud\b`/`\bquad\b` → `Claude`, etc. Grep
for likely homophones of your domain terms and confirm by context before replacing.

## Hosted alternative

ElevenLabs **Scribe** is the hosted STT counterpart to `elevenlabs-say`, but there is
no `ELEVENLABS_API_KEY` in the secret stores yet, so local whisper.cpp is the path.
