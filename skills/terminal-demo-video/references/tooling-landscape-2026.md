# Programmatic terminal/demo video generation: 2026 landscape

Decision-grade comparison for a deterministic, Nix-packageable, regenerable
pipeline producing a flat/monospace/terminal-forward demo. The product is
CLI/dev tooling, so terminal-recording tools are the center of gravity.

## Ranked recommendation

1. **Own it on top of the `tui` PTY driver** (this repo's `reel`). Pure Rust +
   ffmpeg, browser-free, cross-platform, dogfoods the product. Cost: a small
   monospace rasterizer. Chosen here.
2. **vhs + ffmpeg** if you do not own a PTY driver. Native flat-terminal look,
   `.tape` scripts, in nixpkgs. Footgun: headless Chromium, so no pure build.
3. **asciinema + agg + ffmpeg** for the most reproducible external path. GIF-only.
4. **Motion Canvas** only for code-driven title cards beyond ffmpeg. Adds Chromium.
5. **Remotion** disqualified by licensing for a company (free only ≤3 employees).
6. **Rust creative-coding** (nannou / vello / tiny-skia → PNG → ffmpeg) only if
   bespoke generative motion is itself the goal.

## A. Terminal-recording tools

### vhs (charmbracelet)
"Write terminal GIFs as code." Go, MIT, ~19.8k stars, v0.11.0 (Mar 2026).

- Declarative `.tape`: `Output` (`.gif`/`.mp4`/`.webm`/PNG dir), `Type`, `Enter`,
  `Ctrl+...`, `Sleep`, `Wait /regex/` (block until text appears), `Hide`/`Show`
  (off-camera setup), `Screenshot`, `Source`, `Require`, `Env`.
- Aesthetic controls: `Set FontFamily "Berkeley Mono"`, `Set FontSize`,
  `Set LineHeight`, `Set LetterSpacing`, `Set Theme` (built-in or inline base16
  JSON), `Set Padding`/`Margin`/`MarginFill`, `Set BorderRadius 0`,
  `Set WindowBar` off, `Set Width`/`Height`/`Framerate`/`TypingSpeed`/
  `PlaybackSpeed`/`CursorBlink`. No auto light/dark: run two tapes.
- Architecture: runs the program inside **ttyd**, which renders via **xterm.js in
  headless Chromium**, screenshots frames, encodes with **ffmpeg**. So fonts must
  be resolvable by Chromium (a missing font silently falls back), and a browser
  is in the loop even though output looks like a pure terminal.
- Determinism: strong behaviorally (`Wait /regex/` removes timing flakiness;
  marketed for CI golden-file tests). Not byte-identical across machines (font
  rasterization, ttyd/ffmpeg versions); Nix pinning gets most of the way.
- Nix: `vhs`, `ttyd`, `ffmpeg` all in nixpkgs. Package as a `nix run` wrapper with
  the deps + font on PATH, NOT a pure `runCommand` build.
- **Footgun: does not work in a pure Nix build sandbox** (nixpkgs #455564:
  headless Chromium + ffmpeg report `Could not find codec parameters ... png ...
  unspecified size`, empty output). Commit the artifact, regen at dev/CI time.

### asciinema + agg
asciicast v2 is newline-delimited JSON: a header + `[time, "o", data]` events.
`agg` (Rust, GPL-3.0, in nixpkgs) renders a `.cast` to GIF via gifski.

- `asciinema rec demo.cast` captures a real run as editable JSON, then
  `agg demo.cast demo.gif`. Built-in themes + custom hex, configurable font,
  swash/resvg backends.
- Determinism: excellent. The `.cast` is committable, diffable text; agg's render
  is a pure function of cast + theme + font. No browser.
- Tradeoffs: GIF-only (pipe through ffmpeg for MP4/WebP), less window styling,
  GPL-3.0 on agg (matters only if you vendor its code, not for invoking it).

### Also-rans
- termsvg / termtosvg / svg-term: asciicast → animated SVG. GitHub does not
  animate SVG reliably; only if the target switches to inline SVG.
- terminalizer: Node/Electron, heavy, flaky, weaker maintenance. Worse than vhs.
- autocast: scripts asciinema input via YAML; thinner vhs-like layer, smaller
  ecosystem.

Record the real binary, not a scripted fake: both vhs and asciinema make real
runs reproducible, and a faked transcript drifts from real behavior and feels
dishonest for dev tooling. Stub the environment, not the output.

## B. Code-driven 2D animation

### Remotion
React/TS; `useCurrentFrame()` drives a composition; `npx remotion render` renders
via Chrome Headless Shell + bundled ffmpeg to MP4/WebM/GIF/PNG. Determinism good
(frame = pure function of time). **License: free only for an individual or a
for-profit org with up to 3 employees**; above that needs a paid company license.
Disqualified for a company README clip.

### Motion Canvas
TS, generator-based, MIT, built for explainer animations. Renders an image
sequence via Puppeteer/headless Chrome, then a Video (ffmpeg) exporter muxes.
Headless CI works but you package Chromium + ffmpeg. Great for clean flat
motion-graphics title cards; you would hand-build the terminal look. Best beside
a terminal recorder, not as the terminal renderer. ~1-2 days for a polished intro.

### Others
Theatre.js / Rive / Lottie / p5.js / two.js / GSAP-to-video are editor-centric or
need a separate headless-browser capture step, and none give a terminal aesthetic
for free. Not worth it over a recorder + ffmpeg.

### Rust options
- nannou (v0.19, wgpu): creative-coding; capture frames to PNG, encode with
  ffmpeg. Build everything yourself.
- vello/lyon (+ wgpu) or tiny-skia (CPU, best Nix story) → PNG sequence → ffmpeg.
  Maximum determinism, browser-free, but hand-rolled renderer + animation engine.
- Honest cost for a 15-30s clip: 2-5+ days vs an afternoon for vhs, and it will
  not look more "terminal" than vhs. Pick Rust here only when the terminal grid IS
  the content (as in `reel`, which gets the VT grid free from `tui`) or when
  bespoke generative motion is the goal.

## C. Compositing (ffmpeg)

ffmpeg is the universal glue regardless of renderer.

- Title cards/transitions without a second framework: `drawtext` cards + `xfade`
  + `concat`.
- High-quality GIF: two-pass palette
  `fps=...,scale=...:flags=lanczos,split[s0][s1];[s0]palettegen=stats_mode=single[p];[s1][p]paletteuse=dither=floyd_steinberg`.
- Animated WebP: `-vcodec libwebp -loop 0 -lossless 0 -compression_level 6
  -q:v <quality> -preset picture`.
- Dark/light: render twice with two themes, emit both files; do not recolor in
  post.
- Looping: `-loop 0` for GIF/WebP; for MP4 use a clean first/last frame.

What belongs here: format fan-out, palette optimization, light/dark export, title
concat, looping. What does not: the terminal rendering itself.

## D. Hybrid verdict

Real terminal capture for the meat + ffmpeg (not a second framework) for
titles/transitions/format fan-out. Add Motion Canvas only if you later need
animated intros beyond `drawtext`/`xfade`. Avoid a Remotion/Motion-Canvas-for-
everything stack: it doubles the heavy browser dependency for marginal polish.

## Effort estimates

| Path | Effort to a polished README clip | Notes |
|---|---|---|
| `reel` (tui + rasterizer + ffmpeg) | ~1-2 days | Pure, cross-platform, dogfoods the driver; owns a small rasterizer |
| vhs + ffmpeg | ~0.5 day | Native aesthetic; footgun = sandbox/fonts |
| asciinema + agg + ffmpeg | ~0.5-1 day | Most reproducible external; GIF-only |
| Motion Canvas titles + recorder | ~1.5-2 days | Adds headless-Chromium Nix work |
| Remotion | ~1-2 days + license | Licensing disqualifies for a company |
| Rust creative-coding → ffmpeg | ~2-5+ days | Only if generative motion is the goal |

## Sources

- vhs: https://github.com/charmbracelet/vhs (commands, themes/fonts, ttyd+ffmpeg, MIT)
- vhs Nix-sandbox failure: https://github.com/NixOS/nixpkgs/issues/455564
- ttyd / xterm.js (vhs render path): https://github.com/xtermjs/xterm.js , https://www.npmjs.com/package/xterm-headless
- asciicast v2: https://docs.asciinema.org/manual/asciicast/v2/ ; asciinema: https://github.com/asciinema/asciinema
- agg: https://github.com/asciinema/agg
- Remotion render/license: https://www.remotion.dev/docs/cli/render , https://www.remotion.dev/docs/license
- Motion Canvas: https://motioncanvas.io/docs/rendering/video/
- nannou: https://github.com/nannou-org/nannou ; tiny-skia: https://github.com/linebender/tiny-skia
- ffmpeg GIF/WebP: https://blog.pkh.me/p/21-high-quality-gif-with-ffmpeg.html , https://mattj.io/posts/2021-02-27-create-animated-gif-and-webp-from-videos-using-ffmpeg/
