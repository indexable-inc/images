# overlay engine (overlay-core)

`packages/minecraft/bossbar-overlay/app/crates/overlay-core` is the reusable engine behind
every overlay in [bossbar-overlay](overview.md). It owns the mechanics every
overlay shares and none of the domain: a transparent float window, one
textured-quad wgpu pipeline with the Minecraft bitmap font baked in, gesture
disambiguation, a native context menu, headless snapshotting, and shared
animation primitives. A consumer builds a `Vec<Quad>` and hands it to `Gpu::draw`
(live window) or `snapshot::render_to_png` (headless PNG) (`src/lib.rs:16`).

## Modules (`src/lib.rs:21`)

- **`window`** - float-window attributes and surface/adapter plumbing.
- **`gpu`** - the textured-quad pipeline, texture registry, and text layout.
- **`bitmap_font`** - the vanilla `ascii.png` face and its metrics.
- **`gesture`** - press/drag/click and scroll-drag math.
- **`menu`** - a native right-click context menu.
- **`snapshot`** - headless render-to-PNG.
- **`anim`** - easing curves, a hover stepper, a breathe oscillator.

Re-exports: `HoverAnim`, `BitmapFont`, `DragClick`, `scroll_drag_delta`, `Gpu`,
`Quad`, `TexHandle`, `SHADOW`, and the pinned `glam`/`wgpu`/`winit`
(`src/lib.rs:29`), so consumers name the exact versions this workspace pins.

## Float window (`window.rs`)

`float_attributes` builds a transparent, borderless, non-resizable,
`AlwaysOnTop` window sized in physical pixels (`window.rs:193`). The desktop stays
click-through wherever no overlay window sits because there is simply no window
there to intercept the pointer. Surface helpers pick an sRGB format
(`window.rs:213`), a transparent alpha mode (PostMultiplied/PreMultiplied/Inherit,
warning if only Opaque is available, `window.rs:224`), and a FIFO config
(`window.rs:244`). `request_adapter_device` blocks once on first window
(`window.rs:264`).

macOS specifics (winit exposes no API for these, so they drop to AppKit through
the raw window handle):

- `build_event_loop` uses `ActivationPolicy::Accessory` so the HUD takes no Dock
  slot or app-switcher entry (`window.rs:285`).
- `raise_to_front` calls `-[NSWindow orderFrontRegardless]` to raise a hovered
  overlay above same-level siblings without taking keyboard focus
  (`window.rs:308`).
- `enable_background_hover` adds an `NSTrackingArea` with `NSTrackingActiveAlways`
  so a background overlay receives hover while another app is active
  (`window.rs:349`).
- `suppress_scroll_momentum` installs a local `NSEvent` monitor that drops
  momentum-phase scroll events, so a two-finger scroll-drag stops on lift instead
  of coasting (`window.rs:148`).
- `move_window_with_cursor` warps the pointer with `CGWarpMouseCursorPosition`
  anchored to the just-set position, so a fast scroll-drag keeps the pointer glued
  to the window (`window.rs:28`).
- `visible_frame_logical` returns the screen area minus menu bar/Dock for
  auto-placement (`window.rs:409`).

Off macOS these are no-ops or defer to the compositor (X11/Wayland deliver hover
and need no non-activating raise). On wlroots Wayland the boss bar uses a separate
layer-shell backend instead of toplevel windows; see
`crates/bossbar/src/layer_shell.rs:1` in [overview](overview.md).

## wgpu pipeline (`gpu.rs`)

One `RenderPipeline` draws textured quads with alpha blending (`gpu.rs:161`).
Vertices arrive in physical pixels with a top-left origin; the vertex stage
(`src/sprite.wgsl`) converts to clip space using the framebuffer size, so all
layout math stays in pixel units on the CPU, matching how Minecraft blits GUI
sprites (`gpu.rs:3`). The sampler is `Nearest` to keep pixel art crisp when
scaled (`gpu.rs:206`).

- `Quad` (`gpu.rs:48`) is `{tex, x, y, w, h, uv, color}` in physical pixels;
  `uv` is `(u0,v0,u1,v1)` and passing `u0>u1` mirrors horizontally (used for the
  book's right page). `color` is a straight-alpha RGBA tint multiplied into the
  texel, so the 1x1 white texture turns the tint into a flat fill (`gpu.white`,
  `gpu.rs:271`).
- `Gpu::new` registers the white pixel and the embedded font sheet up front
  (`gpu.rs:113`). `register_png`/`register_rgba`/`register_image_scaled` add
  textures and return a `TexHandle` (`gpu.rs:276`); `register_image_scaled` is
  fallible and downscales to a max side, for untrusted-ish images like avatars
  (`gpu.rs:294`).
- `text`/`text_shadow` emit one glyph quad per character through the same
  pipeline, so titles and page text are just more sprites; `text_shadow` draws a
  one-pixel grey `SHADOW` offset first (`gpu.rs:324`, `gpu.rs:339`). `measure`
  sizes text without drawing (`gpu.rs:354`).
- `draw` expands quads to 6 vertices each, clears the target to
  `Color::TRANSPARENT` so the desktop shows through, and draws in submission
  order so later quads layer over earlier ones (`gpu.rs:360`).

## Bitmap font (`bitmap_font.rs`)

The whole text stack: no TTF, no shaper. `ascii.png` is a 128x128 sheet of 8x8
glyph cells indexed by code value, white-on-transparent so the per-vertex color
tints glyphs directly (`bitmap_font.rs:1`). `BitmapFont::from_ascii_rgba`
measures each glyph's inked width as the rightmost inked column + 1, the same
measurement the vanilla client makes; the space glyph's advance is pinned to 4
since it has no ink (`bitmap_font.rs:58`). `shared()` decodes the embedded
`ASCII_PNG` once via `LazyLock` so window-sizing can measure before any GPU
exists, and the live `Gpu` shares the same metrics (`bitmap_font.rs:24`). The
sheet is `include_bytes!`-d from `assets/ascii.png`, supplied by
[minecraft-assets](../minecraft-assets/overview.md).

## Gestures (`gesture.rs`)

`DragClick` watches the cursor and left-button events to disambiguate a press
into a drag or a click: a press defers the decision, `cursor_moved` returns
`true` exactly once when travel crosses the threshold (hand off to
`Window::drag_window`), and `released` returns `true` only if the pointer never
crossed it (a click) (`gesture.rs:47`). `scroll_drag_delta` converts a winit
scroll delta into a logical-point translation; the sign is negated so the window
follows the gesture (grab-and-move), inheriting the user's scroll-direction
preference (`gesture.rs:39`). Trackpad `PixelDelta` is divided by the scale
factor; a notched wheel `LineDelta` steps by `LINE_POINTS=16` (`gesture.rs:21`).

## Menu, snapshot, animation

- **`menu::popup`** builds an `NSMenu`, attaches a tiny `NSObject` target, and
  pops it up at the pointer, blocking until the user picks or dismisses; returns
  the chosen index. A no-op returning `None` off macOS (`menu.rs:17`).
- **`snapshot::render_to_png`** runs the same `Gpu` against an offscreen texture
  and reads it back (handling wgpu's 256-byte row alignment) into a transparent
  PNG, so an always-on-top transparent window is verifiable pixel-for-pixel from
  a file (`snapshot.rs:15`). This backs every app's `--snapshot` flag.
- **`anim`** holds `ease_out_cubic`/`ease_out_back` (`anim.rs:17`), `breathe`
  (a sine oscillator off a continuous clock, `anim.rs:35`), `HoverAnim` (a
  `0..=1` hover amount eased toward a target each frame, `anim.rs:47`), and
  `scale_quads_about` (scale a laid-out group in place for the hover grow,
  `anim.rs:89`). Durations and rationale live in the repo `animation` skill.
