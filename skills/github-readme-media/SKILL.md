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
with no manual browser step**: commit an **animated WebP** and reference it
through a `<picture>` element. This is the only approach that satisfies all four
at once.

```html
<picture>
  <source media="(prefers-color-scheme: dark)"  srcset="docs/demo-dark.webp">
  <source media="(prefers-color-scheme: light)" srcset="docs/demo-light.webp">
  <img alt="demo" src="docs/demo-dark.webp" width="800">
</picture>
```

The paths are repo-relative committed files. The `<img>` is the fallback for
renderers that ignore `<picture>`. WebP does not loop unless the encoder sets it,
so encode with ffmpeg `-loop 0`.

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
