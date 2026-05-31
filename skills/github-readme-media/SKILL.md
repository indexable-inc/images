---
name: github-readme-media
description: Embed autoplaying, looping, dark/light-aware video or animation in a GitHub README. Covers what the GitHub markdown sanitizer actually renders (inline video vs animated WebP/AVIF/GIF), size limits, and the prefers-color-scheme picture swap. Use when adding a demo clip, screencast, or animated banner to a README that must regenerate in CI.
---

# GitHub README media

What GitHub actually renders inside a README, verified live in Chromium 147 (the
engine github.com serves), not just from docs. Full evidence and source URLs:
[`references/findings-2026.md`](references/findings-2026.md).

## The one decision

For a clip that must **autoplay, loop, swap on dark/light, and regenerate in CI
with no manual browser step**: commit an **animated image** and reference it
through a `<picture>` element. Pick the codec by frame rate:

- **AVIF (AV1) is the primary** when the clip is high-fps or detailed. AV1's real
  inter-frame compression makes static hold-frames almost free, so a 60fps
  terminal clip lands around 140 KB where the same WebP is multiple MB. It
  animates on github.com (verified) and in every current browser.
- **WebP is the fallback** (and the `<img>`), thinned to a lower fps so it stays
  under GitHub's 10 MB image cap for renderers that lack AVIF.

```html
<picture>
  <source media="(prefers-color-scheme: dark)"  srcset="docs/demo-dark.avif"  type="image/avif">
  <source media="(prefers-color-scheme: light)" srcset="docs/demo-light.avif" type="image/avif">
  <source media="(prefers-color-scheme: dark)"  srcset="docs/demo-dark.webp">
  <source media="(prefers-color-scheme: light)" srcset="docs/demo-light.webp">
  <img alt="demo" src="docs/demo-dark.webp" width="800">
</picture>
```

The paths are repo-relative committed files. The `<img>` is the fallback for
renderers that ignore `<picture>`. Neither format loops unless the encoder sets
it, so pass ffmpeg `-loop 0`. AVIF is undocumented by GitHub (works when
committed and served via `/raw/`, but is not a valid drag-drop attachment type),
which is why WebP stays as the documented fallback.

ffmpeg gotcha: its AVIF **muxer** writes animation (`-c:v libsvtav1 -loop 0
out.avif`), but its AVIF **decoder** only extracts the first frame, and
`mediainfo`/`ffprobe` misreport AVIF sequences as a single frame. Verify
animation in a real browser, not with those CLIs.

## Why not inline video

- GitHub's markdown sanitizer **strips author-written `<video>` tags entirely**
  (confirmed against the `POST /markdown` API). You cannot hand-author a working
  player.
- The `<video>` player you see in some READMEs is one GitHub *generates* when you
  paste a bare drag-and-drop attachment URL. That upload needs a logged-in
  browser session: there is **no REST/GraphQL API**, so it **cannot run in CI**.
- Even then GitHub forces `controls` on and **ignores `autoplay` and `loop`**.
- A video file **committed to the repo** (relative path or `/raw/` URL) does
  **not** become a player. Only the user-attachments CDN URL does.

So inline video fails both "regenerate in CI" and "autoplay/loop". Use an
animated image instead.

## Format ranking for a terminal/screen demo

| Format | Renders + animates on github.com | Size (same 720p clip) | Notes |
|---|---|---|---|
| Animated **WebP** | yes (verified) | ~2.3 MB | 24-bit color keeps mono text crisp; **recommended** |
| Animated **AVIF** | yes (verified, committed `/raw/`) | ~0.56 MB | Smallest + cleanest, but **undocumented** by GitHub and not a valid drag-drop type; slightly higher "could change" risk |
| Animated **GIF** | yes (documented) | ~8.2 MB | Universal fallback; 256-color cap bands anti-aliased text |
| **APNG** | animates | larger than WebP | No advantage over WebP |

Quality: AVIF ≈ WebP ≫ APNG > GIF. Size: AVIF < WebP < APNG < GIF (GIF ~15× AVIF).

## Limits and gotchas

- Attachment uploads: images/GIFs **10 MB**, video 10 MB free / 100 MB paid,
  other files 25 MB. Files committed to git: warning at **50 MB**, hard block
  over **100 MB** per file (then Git LFS). Keep each `.webp` well under 10 MB; at
  720p it lands far below.
- `<picture>` + `prefers-color-scheme` survives the sanitizer and works with
  animated sources; the animation and the theme selection are independent. It
  does **not** help video, since the `<video>` tag is stripped regardless.
- Generate two theme variants (different terminal background) and commit both.
- This repo's regenerator is [`reel`](../../packages/reel/) (`nix run .#reel`),
  which emits `demo-dark.webp` + `demo-light.webp` exactly for this snippet. See
  [`terminal-demo-video`](../terminal-demo-video/SKILL.md) for how it is built.
