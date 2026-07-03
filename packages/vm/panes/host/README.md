# panes-host

macOS window agent for seamless guest-Linux windows (index#1686): connects to
the guest compositor's stream (unix socket today, fronted by the libkrun vsock
port map later), presents each guest toplevel as a real `NSWindow`, and
forwards input back. The wire contract lives in `packages/vm/panes/protocol`.

## Architecture

Three thread roles, one owner per resource:

- **Main thread (AppKit).** Owns every window and all state: `app::APP` is a
  main-thread `thread_local` holding the window map, the shared Metal
  renderer, and the outgoing sender. All AppKit/CoreAnimation calls happen
  here (vmkit's `MainThreadMarker` discipline). AppKit calls that
  synchronously re-enter delegates (`close`, `makeKeyAndOrderFront`) are
  deferred until the state borrow is released.
- **Supervisor/reader thread.** Owns the socket. Connects with backoff
  (250ms doubling to 5s), refuses a mismatched protocol major and hangs up,
  then decodes `ToHost` frames and `dispatch_async`s them onto the main
  queue. On disconnect every guest window closes.
- **Writer thread.** Drains an mpsc of `ToGuest` messages into the socket
  (batch per flush), so the main thread never blocks on a stalled guest.

Per `WindowNew` the host builds: a titled/closable/miniaturizable/resizable
`NSWindow` (content size = buffer px / scale), an input `NSView` subclass
hosting a `CAMetalLayer` (`framebufferOnly`, `displaySyncEnabled = false`:
synced presents measured a constant ~40ms tick-to-glass through the windowed
compositing path vs ~24-32ms immediate, and presents stay tick-paced so
nothing free-runs (index#1686); `maximumDrawableCount = 2`,
`contentsScale = backingScaleFactor`), two
surface `MTLTexture`s (double-buffered: `replaceRegion` does not synchronize
against GPU access, so uploads must never touch the texture a still-executing
present is sampling), and a per-window `CAMetalDisplayLink`. The window is
only ordered front on its first `WindowFrame`, so an empty window never
flashes.

### Frame path and the ack loop

`WindowFrame` tiles are decoded on the main thread (LZ4 via `lz4_flex`) into
a per-window damage log (`full` frames blank uncovered pixels first). The
window is then dirty. On the presenting tick the log is `replaceRegion`ed
into whichever texture is not in flight (each texture replays the damage it
missed while the other was on screen), and the layer flips to it.

`CAMetalDisplayLink` (macOS 14+, from `objc2-quartz-core` 0.3, added to the
main run loop in common modes with `preferredFrameRateRange` pinned to the
window's own panel max rate -- min == max == preferred, recomputed on
`windowDidChangeScreen:`; the adaptive 60..120 range measured downshift
stretches on borderline streams, index#1686) ticks at the panel rate and
hands us the drawable. If
the window is dirty we encode one fullscreen-triangle pass sampling the
texture into the drawable (a render pass, not a blit, because
`framebufferOnly` drawables are render-target-only, and sampling stretches
stale content for free during resize), present, and only then send
`ToGuest::Ack { id, seq }`. The compositor fires Wayland frame callbacks off
that ack, so guest rendering is genlocked to ProMotion instead of running an
open-loop timer. Coalescing: if several frames land between ticks, only the
newest is presented and acked; guests should treat an ack as "presented up
to seq". A frame the host cannot take at all (zero-size, texture allocation
failure) is still acked immediately so the guest's one-in-flight loop never
wedges on it. The link starts paused, unpauses on content/resize, and
re-pauses after ~250ms of idle ticks so a quiet window stops costing CPU.

`PANES_TRACE=1` emits one parseable stderr line per input event, frame
ingest, and present (plus `MTLDrawable.presentedTime` glass ground truth),
all on the `NSEvent.timestamp` clock; `tools/latency_probe.py` is the
matching synthetic guest that drives ack-paced load and reports present RTT
percentiles. Together they are the before/after evidence for the numbers
above (index#1686).

### Resize

`windowDidResize` sends `ToGuest::Configure` (view bounds x
`backingScaleFactor`) and marks the window dirty so the next tick redraws
immediately, stretching the old texture until the matching-size frame
arrives. During live resize the layer runs `presentsWithTransaction` so the
present rides the same CATransaction as the window frame (no edge shimmer);
outside it the async present path is lower-latency. `windowShouldClose`
never closes locally: it sends `CloseRequest` and the window dies when
`WindowGone` comes back (the WSLg lesson: window existence is owned by the
compositor).

## Input mapping

- **Keyboard**: `keyDown`/`keyUp` map `NSEvent.keyCode` (kVK) to evdev codes
  via `src/keymap.rs`, generated from the keycodemapdb project by
  `tools/gen_keymap.py` (same dataset QEMU/libvirt use). `isARepeat` events
  are dropped: guests auto-repeat from `wl_keyboard.repeat_info`, and the
  user's actual macOS repeat timing is shipped once per connection
  (`ToGuest::KeyRepeat`, from `NSEvent.keyRepeatDelay/Interval`; protocol
  1.2) so guest repeat matches System Settings exactly.
  `flagsChanged` turns into modifier press/release by toggling a held-set
  keyed on kVK; caps lock (one event per toggle) synthesizes press+release.
  Forwarded key presses are tracked in a second held-set: keyUps only go
  out for tracked keys, and on resign-key every held key and modifier is
  released guest-side (AppKit stops delivering keyUp/flagsChanged to a
  non-key window; a key stuck down guest-side would auto-repeat forever).
  The `NSWindow` subclass reroutes Cmd keyUps to the view -- AppKit
  swallows them as key-equivalent processing, which used to stick e.g.
  Cmd-Backspace down forever guest-side.
  Cmd+W (CloseRequest) and Cmd+Q (CloseRequest to all, then exit) stay
  host-side; other Cmd chords are forwarded.
- **Pointer**: the view is flipped (top-left origin) and multiplies points
  by `backingScaleFactor`, so protocol coordinates are buffer pixels.
  Buttons map to evdev (`BTN_LEFT` 0x110, `BTN_RIGHT` 0x111, `BTN_MIDDLE`
  0x112, side/extra 0x113/0x114); a motion is sent before each button so the
  press lands where the user clicked. `acceptsFirstMouse` is true (the
  Parallels/VMware convention) so the activating click reaches the guest.
- **Scroll**: precise deltas (trackpad) become `AxisSource::Finger` pixel
  deltas x scale; wheel lines become `AxisSource::Wheel` with 15 axis units
  per detent plus `v120 = delta * 120`. Both axes are negated
  (`scrollingDelta*` is positive-up, Wayland axis is positive-down). A
  momentum-phase end sends an axis `stop`.
- **Pointer lock** (index#1724): on `ToHost::PointerLock { locked: true }`
  for a window that is key (and while the app is active) the host captures
  the mouse: `NSCursor.hide()` +
  `CGAssociateMouseAndMouseCursorPosition(false)` park the cursor while
  `NSEvent` deltas keep flowing, and the view forwards them as
  `ToGuest::PointerRelative` (x scale) instead of absolute motion. The
  capture's release lives in a `Drop` impl and `sync_capture` is called on
  every transition -- unlock message, resign-key, window close, app
  deactivate, disconnect, Cmd+Q -- so no path leaves the user's cursor
  dissociated. A lock for a non-key window is remembered (`wants_lock`) and
  engages when the window becomes key. AppKit delivers each mouseMoved twice
  (first responder + tracking area); relative deltas dedupe by event
  identity, absolute coordinates never cared. macOS spontaneously unhides a
  hidden cursor on paths of its own (right-mouse-down menu preparation --
  holding right-click in a pointer-locked game showed the cursor -- plus
  screenshot mode and dock hover, glfw#2648/#2656), so while captured every
  button event re-checks `CGCursorIsVisible` and re-hides, immediately and
  once more from the back of the main queue; release unhides until the
  cursor is actually visible, so the (undocumented) hide-nesting counter can
  never strand a hidden cursor.

## Mock guest

`panes-host --mock` serves a built-in mock guest on a temp socket and
connects to it: one 800x600-point toplevel with a moving gradient and a
blocky frame counter, full-damage-tiled in 256px tiles, LZ4 when the host
advertises it. It renders the next frame when the previous seq is acked
(exactly one frame in flight, like the real compositor) and logs every input
event plus a once-a-second ack rate. A right-click into the window toggles
`ToHost::PointerLock`, exercising the host's cursor capture without a VM:
the cursor hides and freezes, motion arrives in the log as `PointerRelative`
deltas, a second right-click releases it:

```
panes-host --mock                       # everything in one process
panes-host --mock-serve /tmp/g.sock     # headless mock only (any OS)
panes-host --connect /tmp/g.sock        # host against an external socket
panes-host --tcp 127.0.0.1:7100         # TCP instead of unix (debugging)
```

Measured on this machine (M-series, ProMotion at
`maximumFramesPerSecond = 120`, scale 2): steady **120.0 acks/s** at a
1600x1192 px buffer, host ~11% of one core, mock guest ~17% (render + LZ4
dominate). The rate held at 120 up to the largest buffer AeroSpace tiled it
to (1370x2516 px).

## Regenerating the keymap

```
python3 tools/gen_keymap.py > src/keymap.rs
```

The script fetches `data/keymaps.csv` from
<https://gitlab.com/keycodemap/keycodemapdb> (or takes a local path); the
output is committed so builds never touch the network.
