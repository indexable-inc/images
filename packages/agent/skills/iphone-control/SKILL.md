---
name: iphone-control
description: "Drive a physical iPhone/iPad (over USB cable or Wi-Fi) from the kernel iphone helper: screenshots, app launch, GPS, and real taps/typing via WebDriverAgent. Use when controlling a physical iOS device, automating an app, requesting a ride, or filling a form on a real iPhone, including cable-free over the network. Covers the one-time human gates, going wireless, and the WDA gotchas."
---

## iPhone control

The kernel `iphone` helper (pymobiledevice3 9.27.0) drives a physical device
over USB **or** Wi-Fi. Start every task with `await iphone.doctor()` — it names
exactly which prerequisite is missing instead of a multi-step debug.

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

### Wireless (no cable)

The helper is transport-agnostic: every call goes through `pymobiledevice3
usbmux ...` and the `tunneld` daemon, both of which macOS serves over USB **and**
Wi-Fi. `devices()` lists a network-connected device the same as a USB one (it
even dedupes a device that shows up on both, see the `_one_device` comment). The
WDA forward's local end is always `127.0.0.1:8100`; usbmux routes it to the
device whether that hop is USB or Wi-Fi. So no code path is cable-specific.

What you must do **once over the cable** to bootstrap (wireless cannot pair from
nothing):

1. Plug in and **Trust** the device (the pairing record persists on the Mac).
2. Enable **Connect via network**: Xcode > Window > Devices & Simulators >
   select the device > check "Connect via network". This is what makes macOS
   `usbmuxd` advertise the device over the LAN.
3. The usual one-time gates below (Developer Mode, Apple ID in Xcode,
   `wda_build_install()`).

Then unplug. On the same LAN the device stays reachable; for off-LAN, put the
Mac and the iPhone on the same Tailscale network so the usbmux/tunnel hops
resolve. Verify with `await iphone.devices()` (the row's `ConnectionType` reads
`Network`) and proceed exactly as over USB.

What is solid vs fragile wirelessly:

- **Solid**: inventory (`devices`/`info`/`apps`), plus WDA UI control (`source`,
  `tap`, `type_text`, the WDA screenshot) **once WDA is already running** and the
  usbmux forward is established. These ride lockdown and the WDA HTTP forward.
- **Fragile**: any call that starts or uses DVT developer services needs the
  RemoteXPC tunnel, and `tunneld` over Wi-Fi is less reliable than over USB on
  iOS 17+. That includes `wda_start` itself: it launches the XCUITest runner via
  `developer dvt xcuitest` before opening the forward, so the WDA *startup* rides
  the fragile path even though the taps that follow do not. Also `launch`, the
  DVT `screenshot`, `simulate_location`, and the DDI mount. If one of these hangs
  or fails wirelessly, re-tether for that step (notably the one-time
  `wda_build_install` and each `wda_start`), then run the taps over Wi-Fi.

**iPhone Mirroring is not a substitute**: it is a consumer feature with no API
or CLI, room-range only (Bluetooth + same Apple ID, phone locked). Automating it
would mean screen-scraping the Mac mirror window and posting synthetic events,
which is far more brittle than the WDA path above. Use Wi-Fi WDA, not Mirroring.

### Safety

`start_tunneld` / `wda_start` run a **root** daemon — they require `sudo=True`
explicitly; nothing else escalates. When automating a purchase/dispatch (e.g. a
ride), drive to the confirm screen and **stop** — let the human tap the final
Confirm.
