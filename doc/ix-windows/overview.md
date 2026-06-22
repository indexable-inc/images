# ix-windows

`packages/ix-windows` renders each live MCP resource as its own floating,
blurred **overlay** webview window that auto-sizes to its content. It is a second
consumer of the dashboard producer stream, alongside the
[dashboard](../dashboard/overview.md) web aggregator: instead of folding
producers into a web board, it maps each `resource/<id>` html pane to one OS
window. A window opens when a resource appears, re-renders in place on update (no
reload, so scroll and focus survive), and closes when the resource closes or its
producer disconnects.

It reuses [dashboard-core](../dashboard-core/overview.md) for the entire
transport, so the MCP needs no change: the MCP already publishes every
`register_resource()` view as an [`HtmlView`] pane keyed `resource/<id>` (see
`packages/mcp/ix_notebook_mcp/pane_bridge.py`), and this process just windows
those panes. A producer's exec runs, namespace, and cells stay on the web
canvas.

It is split into a reusable engine library (`src/lib.rs`, `[lib] name =
"ix_windows"`) and a thin binary (`src/main.rs`, `[[bin]] name = "ix-windows"`).

## Build and run

Rust workspace package (`packages/ix-windows/Cargo.toml`), built as the
`ix-windows` flake output (`package.nix`: `flake = true`). Deps: `dashboard-core`,
`clap`, `tao`, `wry`, `tokio`, `serde_json`; on macOS also `objc2` +
`objc2-app-kit` + `objc2-foundation` + `objc2-web-kit` for the native blur and
WebKit tuning.

```
nix run .#ix-windows                 # watch the default discovery dir
nix run .#ix-windows -- --dir /tmp/ixw
nix build .#ix-windows
```

**Darwin-only flake output.** `wry` links the system WebKit framework on macOS;
Linux (WebKitGTK) is a later add, so `default.nix` restricts `meta.platforms` to
`aarch64-darwin` and `x86_64-darwin`. The native blur and WebKit tuning are
behind `#[cfg(target_os = "macos")]`, so the crate still compiles on other
targets, just without that styling (no blur, no auto-resize is unaffected since
it is plain `tao`).

## CLI (`src/main.rs`)

| flag | default | meaning |
| --- | --- | --- |
| `--dir` | discovery dir | producer-socket directory to watch, matching the `dashboard` aggregator. Defaults via [`discovery_dir`](../dashboard-core/overview.md#discovery-paths). |
| `--rescan-ms` | `500` | how often to rescan the directory for new/removed sockets. |

## Threading model (`src/main.rs`)

Windows must be created and driven on the main thread (`tao`), but the
subscriber is async, and the page's measuring script also feeds events back. The
binary:

1. Builds a `tao` `EventLoop` parameterized on [`UserEvent`] as its user-event
   type, and a proxy. [`UserEvent`] is one of a `ProducerEvent`
   (`UserEvent::Producer`), a content-size report (`UserEvent::Resize`), or a
   move request (`UserEvent::Drag`, posted when the user presses the card chrome).
2. Spawns a side thread running a multi-thread tokio runtime that calls
   [`subscribe`](../dashboard-core/overview.md#consumer-side-subscribe-srcsubscribers)
   and forwards each event as `UserEvent::Producer` into the loop via a clone of
   the proxy; it stops when the loop has exited.
3. Runs the event loop with `ControlFlow::Wait` (reactive viewer, not an
   animation loop) and dispatches to a [`WindowManager`]:
   `Producer(Snapshot)` -> `apply_snapshot`, `Producer(Gone)` -> `producer_gone`,
   `Resize { window, .. }` -> `resize`, `Drag { window }` -> `begin_drag`,
   `WindowEvent::CloseRequested` -> `window_closed`.

## The engine: `WindowManager` (`src/lib.rs`)

[`WindowManager`] owns the open windows and reconciles them against each producer
snapshot. The window-creation path is generic over the loop's user-event type
`T` (it only needs the `EventLoopWindowTarget<T>` to build windows), but the
manager also emits [`UserEvent::Resize`] through the loop proxy, so it holds an
`EventLoopProxy<UserEvent>` and is constructed with one. A window's global
identity is the `PaneKey = (producer id, pane id)`, since a pane id is unique
only within its producer.

Public surface:

- [`WindowManager::new`]`(proxy)`: an empty manager that emits resize events
  through `proxy`.
- [`apply_snapshot<T>(target, snapshot)`]: for each pane that is an `Html` view
  whose id starts with `resource/` (`RESOURCE_PREFIX`), refresh an open window or
  open a new one; close windows for this producer's resources no longer present;
  and forget dismissals for resources that vanished. Non-html or non-`resource/`
  panes are skipped, so exec/namespace/cell panes stay on the web canvas.
- [`resize(window, width, height)`]: fit the overlay window to the natural pixel
  size its content reported, clamped to the window's monitor work area; a report
  within 1px of the last applied size is ignored, which also breaks any
  resize/reflow loop.
- [`producer_gone(producer)`]: drop every window of a disconnected producer and
  clear its dismissals.
- [`begin_drag(window)`]: begin an interactive move of the window whose chrome the
  user pressed (`OUTER_JS` posts `"drag"` on mousedown over the card), by calling
  `drag_window`. A borderless, non-resizable window has no native title bar, so
  this is how the overlay is moved.
- [`window_closed(window_id)`]: the user closed a window; remove it and record
  the dismissal. Returns whether it was one of ours.
- [`is_empty`]: whether any resource windows are open.

### Reconcile invariants

- **In-place refresh.** `OpenWindow::refresh` swaps the sandboxed iframe's
  `srcdoc` (`#ix-frame`) via `evaluate_script` when the html changed, and resets
  the title when it changed, so the trusted outer document, the window, and focus
  survive an update (scroll position inside the iframe resets, the trade for never
  running producer script in the trusted document). The new inner document is
  injected as a `serde_json::to_string` JS string literal, so arbitrary resource
  HTML is escaped safely; the iframe sandbox (not that escaping) is what contains
  any script in the body. The iframe's own `ResizeObserver` notices the resulting
  size change and reports it, driving a `resize`.
- **Dismissal tracking.** `dismissed` records resources the user closed while
  still live. Without it, the next snapshot (any content change republishes one)
  would find the window gone and re-open it, fighting the user. It is cleared
  when the resource actually vanishes or its producer disconnects, so a genuine
  re-registration opens a fresh window.
- **Reverse index.** `by_window` maps an OS `WindowId` back to its `PaneKey` for
  close and resize events.
- **Cascade.** `opened` offsets each new overlay so several do not stack exactly
  on top of each other.

## Overlay styling and auto-size

- **Floating overlay.** Windows are built `with_decorations(false)`,
  `with_transparent(true)`, `with_always_on_top(true)`. On macOS the window also
  joins all spaces and floats over fullscreen apps (`NSWindowCollectionBehavior`).
- **Blur behind.** `install_blur` (macOS) inserts an `NSVisualEffectView`
  (`HUDWindow` material, `BehindWindow` blending) as the content view's first
  subview, beneath the transparent webview, with a rounded, shadowed layer. The
  rendered HTML paints on top of it.
- **Sandboxed shell.** `shell(title, body)` builds a fully transparent trusted
  outer document whose `#ix-root` panel (the `STYLE` constant) shrink-wraps a
  single child: a `sandbox="allow-scripts"` (no `allow-same-origin`) `<iframe>`
  (`#ix-frame`) whose `srcdoc` is the resource body wrapped by `inner_document`.
  The producer HTML therefore runs in an opaque origin with no access to the
  trusted document, `window.ipc`, cookies, or storage -- the same trust model as
  the web dashboard's html pane. The opaque origin removes same-origin `fetch`,
  cookies, and storage (it is not a network block: absolute HTTPS subresources may
  still load subject to CORS, though an ES-module `import` from a CDN was observed
  to fail). For a reproducible offline pane the body should be self-contained, so
  anything needing a library is pre-rendered (e.g. mermaid -> SVG) and embedded.
- **Auto-size to content.** `INNER_JS` (inside the iframe) runs a `ResizeObserver`
  on `#ix-content` and `postMessage`s the measured size out; `OUTER_JS` (the
  trusted document) validates that message, sizes the iframe, then posts the
  card's `offsetWidth`x`offsetHeight` over `wry`'s IPC channel (coalesced per
  frame, deduped). The IPC handler forwards it as `UserEvent::Resize`, and
  `WindowManager::resize` fits the OS window. `#ix-content` is `width: max-content`
  so its intrinsic size does not depend on the window width, which keeps the
  measurement stable (no resize loop).
- **Move by chrome.** `OUTER_JS` also listens for a primary-button `mousedown` on
  `#ix-root`; a press that reaches the trusted document landed on the card chrome
  (the iframe captures its own events), so it posts `"drag"`, which the IPC
  handler turns into `UserEvent::Drag` -> `begin_drag` -> `drag_window`. Producer
  content inside the iframe stays interactive.
- **120Hz.** `enable_high_refresh` disables WebKit's private
  `PreferPageRenderingUpdatesNear60FPSEnabled` experimental feature via the
  private `_setEnabled:forExperimentalFeature:` selector (gated by
  `respondsToSelector:` checks), so the webview renders at the display's native
  rate (ProMotion) instead of the ~60fps cap. Best-effort: an OS without those
  selectors stays at the default.

## Tests (`src/lib.rs`)

`shell` wraps the body in `#ix-root` and embeds the measuring script and escapes
the title; `escape_text` covers `<`, `&`, `>`; `parse_size` reads the
`"<w>x<h>"` IPC body. The window/webview/blur paths require a display and are
exercised manually.

[`HtmlView`]: ../dashboard-core/overview.md#wire-types-srcpanrs
[`WindowManager`]: #the-engine-windowmanager-srclibrs
[`UserEvent`]: #threading-model-srcmainrs
[`UserEvent::Resize`]: #threading-model-srcmainrs
[`WindowManager::new`]: #the-engine-windowmanager-srclibrs
[`apply_snapshot<T>(target, snapshot)`]: #the-engine-windowmanager-srclibrs
[`resize(window, width, height)`]: #the-engine-windowmanager-srclibrs
[`producer_gone(producer)`]: #the-engine-windowmanager-srclibrs
[`window_closed(window_id)`]: #the-engine-windowmanager-srclibrs
[`is_empty`]: #the-engine-windowmanager-srclibrs
