---
name: terminal-demo-video
description: Programmatically generate a deterministic terminal demo video/animation for a README or docs page, regenerable from one command. Covers the tooling landscape (vhs, asciinema+agg, Remotion, Motion Canvas, Rust) and this repo's own dogfooded approach built on the tui PTY driver. Use when building or updating an animated CLI demo, screencast, or hero clip.
---

# Terminal demo video

How to generate a short (~15-30s) terminal demo as code, deterministically, with
a flat/minimal/monospace aesthetic. Full tooling comparison with sources:
[`references/tooling-landscape-2026.md`](references/tooling-landscape-2026.md).
Where the rendered clip goes (GitHub README formats, dark/light): the
[`github-readme-media`](../github-readme-media/SKILL.md) skill.

## This repo's approach: dogfood the PTY driver

The repo already owns the hard parts of recording a terminal: [`tui`](../../packages/tui/)
spawns a real PTY and feeds output through the ghostty VT engine
([`vt`](../../packages/vt/)), exposing a rendered grid of `StyledCell`s
(`character`, `fg`, `bg`, `bold`, `italic`, `underline`, `inverse`) plus the
cursor via `read_cursor()`. So instead of depending on an external recorder, the
[`reel`](../../packages/reel/) tool drives a real CLI through `tui`, samples the
styled grid over time, rasterizes each frame to a PNG with a flat palette and a
vendored OFL mono, and muxes the frames to animated WebP (dark + light) with
ffmpeg.

```sh
nix run .#reel            # regenerate docs/demo-{dark,light}.webp
```

Why build our own instead of using vhs (the obvious off-the-shelf choice):

- **Pure and browser-free.** vhs renders through ttyd + xterm.js in headless
  Chromium, so it cannot run in a Nix build sandbox (nixpkgs #455564) and risks
  silent font fallback. `reel` is pure Rust + ffmpeg: deterministic, no browser.
- **Cross-platform regeneration.** It uses the `tui` Rust crate directly, so
  `nix run .#reel` works on macOS and Linux. The Python bindings (`tui-py`) are
  Linux-only, so a Python recorder would not regenerate on a Mac.
- **It dogfoods the product.** The repo's pitch is "drive any terminal program";
  filming the demo with that same driver is the point.

The cost is owning a small rasterizer. For monospace terminal content this is
contained: fixed cell advance, per-glyph rasterization, no text shaping.

## When to reach for an off-the-shelf tool instead

- **vhs** (charmbracelet, in nixpkgs) is the fastest path to a polished terminal
  clip if you accept the headless-Chromium dependency and a `nix run` wrapper
  (not a pure build). `.tape` scripts, themeable, `Set FontFamily`, outputs
  GIF/MP4/WebM. Good default for a repo that does not already own a PTY driver.
- **asciinema + agg** is the most reproducible external path: capture a real run
  to a committable JSON `.cast`, render to GIF in Rust (no browser). GIF-only,
  less window styling.
- **Remotion** is the most powerful code-driven option but is **license-blocked
  for companies** (free only for orgs of ≤3 employees). Do not use it here.
- **Motion Canvas** (TS, MIT) for code-driven motion-graphics title cards, only
  if ffmpeg `drawtext`/`xfade` is not enough; it re-introduces headless Chromium.
- **3Blue1Brown's manim** is a math-animation tool, not a fit for a flat
  terminal-forward dev-tools demo. Avoid.

## Pipeline rules that hold regardless of tool

- **Record the real binary, not a faked transcript.** Drive the actual CLI and
  block on output (`tui` `read_blocking` / vhs `Wait /regex/`), so the demo
  cannot drift from real behavior. Stub the environment, never the output.
- **ffmpeg owns format fan-out and titles.** PNG frames or an intermediate MP4 ->
  animated WebP (`-loop 0`, required for looping) in dark + light; `drawtext`
  title cards and `xfade` transitions instead of a second animation framework.
- **Aesthetic = flat.** Square corners, no glow/gradient, muted palette, a real
  mono. Match the terminal-theme light/dark split
  ([`terminal-theme`](../../packages/terminal-theme/)).
- **Determinism.** Pin tool + font versions through Nix; vendor the font (OFL) so
  the render does not depend on a system-installed face. Berkeley Mono is the
  user's local face but is proprietary, so it is an opt-in `--font` override, not
  the reproducible default.
