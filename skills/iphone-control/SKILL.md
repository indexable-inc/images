---
name: iphone-control
description: "Drive a USB-connected iPhone/iPad from the kernel iphone helper: screenshots, app launch, GPS, and real taps/typing via WebDriverAgent. Use when controlling a physical iOS device, automating an app, requesting a ride, or filling a form on a real iPhone. Covers the one-time human gates and the WDA gotchas."
---

## iPhone control

The kernel `iphone` helper (pymobiledevice3 9.27.0) drives a USB-connected
device. Start every task with `await iphone.doctor()` — it names exactly which
prerequisite is missing instead of a multi-step debug.

### What works without WebDriverAgent

Apple exposes developer "instrument" services over the tunnel, so these are direct:

- `await iphone.devices()` / `info()` / `apps()` — inventory to polars (no tunnel needed)
- `await iphone.start_tunneld(sudo=True)` — the root tunnel daemon (iOS 17+ needs it)
- `await iphone.ensure_developer_ready()` — mount the Developer Disk Image
- `await iphone.screenshot()` — PIL image; `launch(bundle_id)`; `simulate_location(lat, lon)`

### Taps/typing need WebDriverAgent

iOS has **no developer service that synthesizes a touch** — only Apple's XCTest
can, and WebDriverAgent (WDA) wraps XCTest as an HTTP server. So tapping needs WDA.

- `await iphone.wda_start(sudo=True)` — launches the WDA runner + a usbmux forward, returns when ready
- `await iphone.source()` — accessibility tree as polars (name/label/type/x/y/width/height); **this is ground truth** — filter it to find a control, tap its center
- `await iphone.tap(x, y)` / `tap_element(name=...)` / `swipe(...)` / `type_text(...)` / `press("home")` / `home()` / `unlock()`

Typical loop: `tap` a field → `type_text` → `await iphone.source()` to read results → `tap` the result.

### Two one-time human gates (cannot be automated)

1. **Developer Mode** — Settings > Privacy & Security > Developer Mode, toggle on,
   reboot, confirm with passcode. `ensure_developer_ready` raises a clear error if off.
2. **Apple ID in Xcode** — Xcode > Settings > Accounts > + > Apple ID. Needed so
   `wda_build_install` can sign WDA. `doctor()` reports "signing team" once present.

After both, `await iphone.wda_build_install()` builds+signs+installs WDA once
(needs full Xcode at `/Applications/Xcode.app`). Then `wda_start` each session.

### Gotchas (learned the hard way)

- **`xcode-select` vs full Xcode**: if it points at CommandLineTools, `xcrun
  devicectl` is missing and installs fail with "Install Application not available".
  `wda_build_install` switches it to full Xcode automatically.
- **CoreDevice install is unsupported on current iOS** ("capability not supported
  by this device"): install the signed `.app` via the classic installation-proxy
  (`pymobiledevice3 apps install <app>`), not `devicectl`/`xcodebuild test`. The
  helper does this.
- **Black screenshots during automation**: while an XCUITest session holds the
  display, the DVT screenshot returns a black frame, and GPU/Metal views (maps)
  never capture. `iphone.screenshot()` auto-switches to the WDA screenshot when WDA
  is up; for non-UIKit content, rely on `source()` (text/rects) instead of pixels.
- **Drive WDA with W3C Actions**, not pymobiledevice3's `developer wda tap` (its
  tap endpoint 404s on WDA 14.x and by-name lookups go stale). The helper already
  uses W3C actions over a forwarded `:8100`.
- **Coordinates are points**, not screenshot pixels (≈ pixels / device scale).
  `source()` rects are already in points; tap their centers.

### Safety

`start_tunneld` / `wda_start` run a **root** daemon — they require `sudo=True`
explicitly; nothing else escalates. When automating a purchase/dispatch (e.g. a
ride), drive to the confirm screen and **stop** — let the human tap the final
Confirm.
