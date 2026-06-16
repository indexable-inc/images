# ix-windows

`packages/ix-windows` renders each live MCP resource as its own borderless,
square, ghostty-styled native webview window. It is a second consumer of the
dashboard producer stream, alongside the [dashboard](../dashboard/overview.md)
web aggregator: instead of folding producers into a web board, it maps each
`resource/<id>` html pane to one OS window. A window opens when a resource
appears, re-renders in place on update (no reload, so scroll and focus survive),
and closes when the resource closes or its producer disconnects.

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
`objc2-foundation` + `objc2-web-kit` for the WebKit/window tuning
(`Cargo.toml:31`).

```
nix run .#ix-windows                 # watch the default discovery dir
nix run .#ix-windows -- --dir /tmp/ixw
nix build .#ix-windows
```

**Darwin-only flake output.** `wry` links the system WebKit framework on macOS;
Linux (WebKitGTK) is a later add, so `default.nix` restricts `meta.platforms` to
`aarch64-darwin` and `x86_64-darwin` (`packages/ix-windows/default.nix:11`). The
macOS WebKit tuning (`src/lib.rs:294`) is behind `#[cfg(target_os = "macos")]`,
so the crate still compiles on other targets, just without that tuning.

## CLI (`src/main.rs:22`)

| flag | default | meaning |
| --- | --- | --- |
| `--dir` | discovery dir | producer-socket directory to watch, matching the `dashboard` aggregator. Defaults via [`discovery_dir`](../dashboard-core/overview.md#discovery-paths) (`main.rs:28`). |
| `--rescan-ms` | `500` | how often to rescan the directory for new/removed sockets (`main.rs:33`). |

## Threading model (`src/main.rs:37`)

Windows must be created and driven on the main thread (`tao`), but the
subscriber is async. The binary:

1. Builds a `tao` `EventLoop` parameterized on `ProducerEvent` as its user-event
   type, and a proxy (`main.rs:43`).
2. Spawns a side thread running a multi-thread tokio runtime that calls
   [`subscribe`](../dashboard-core/overview.md#consumer-side-subscribe-srcsubscribers)
   and forwards each `ProducerEvent` into the event loop via the proxy; it stops
   when the loop has exited (`main.rs:49`).
3. Runs the event loop with `ControlFlow::Wait` (reactive viewer, not an
   animation loop) and dispatches to a [`WindowManager`] (`main.rs:65`):
   `UserEvent(Snapshot)` -> `apply_snapshot`, `UserEvent(Gone)` ->
   `producer_gone`, `WindowEvent::CloseRequested` -> `window_closed`.

## The engine: `WindowManager` (`src/lib.rs`)

[`WindowManager`] (`lib.rs:69`) owns the open windows and reconciles them against
each producer snapshot. It is decoupled from the event source and generic over
the loop's user-event type `T` so an embedder can drive it from its own `tao`
loop; the binary's `main` is a thin wrapper. A window's global identity is the
`PaneKey = (producer id, pane id)` (`lib.rs:35`), since a pane id is unique only
within its producer.

Public surface:

- [`WindowManager::new`] (`lib.rs:89`) / `Default`: an empty manager.
- [`apply_snapshot<T>(target, snapshot)`] (`lib.rs:99`): for each pane that is an
  `Html` view whose id starts with `resource/` (`RESOURCE_PREFIX`, `lib.rs:31`),
  refresh an open window or open a new one; close windows for this producer's
  resources no longer present; and forget dismissals for resources that vanished.
  Non-html or non-`resource/` panes are skipped, so exec/namespace/cell panes
  stay on the web canvas.
- [`producer_gone(producer)`] (`lib.rs:141`): drop every window of a disconnected
  producer and clear its dismissals.
- [`window_closed(window_id)`] (`lib.rs:157`): the user closed a window; remove
  it and record the dismissal. Returns whether it was one of ours.
- [`is_empty`] (`lib.rs:168`): whether any resource windows are open.

### Reconcile invariants

- **In-place refresh.** `OpenWindow::refresh` (`lib.rs:51`) swaps only the
  `#ix-root` inner HTML via `evaluate_script` when the html changed, and resets
  the title when it changed, so the document, scroll position, and focus survive
  an update (a full reload would flicker and reset them). The new HTML is injected
  as a `serde_json::to_string` JS string literal, so arbitrary resource HTML is
  escaped safely (`lib.rs:55`).
- **Dismissal tracking.** `dismissed` (`lib.rs:79`) records resources the user
  closed while still live. Without it, the next snapshot (any content change
  republishes one) would find the window gone and re-open it, fighting the user.
  It is cleared when the resource actually vanishes or its producer disconnects,
  so a genuine re-registration opens a fresh window.
- **Reverse index.** `by_window` (`lib.rs:73`) maps an OS `WindowId` back to its
  `PaneKey` for close events.
- **Cascade.** `opened` (`lib.rs:83`) offsets each new window so they do not
  stack exactly on a plain desktop; a tiling WM ignores the position hint.

## Native window styling (macOS)

- **Borderless, square corners.** Windows are built `with_decorations(false)`,
  `with_transparent(true)`, 720x480 logical (`lib.rs:194`), exactly like
  ghostty's `window-decoration = none`. The macOS window server only rounds
  *titled* windows, so dropping decorations gives square corners for free.
- **Ghostty-flavored shell.** `shell(title, body)` (`lib.rs:249`) wraps the
  resource HTML in a dark, monospace, Catppuccin-ish document whose `#ix-root`
  holds the body; the `<title>` is escaped, the body is injected verbatim (the
  same trust model as the web dashboard's sandboxed html pane). Styling is the
  `STYLE` constant (`lib.rs:270`).
- **120Hz.** `enable_high_refresh` (`lib.rs:294`) disables WebKit's private
  `PreferPageRenderingUpdatesNear60FPSEnabled` experimental feature via the
  private `_setEnabled:forExperimentalFeature:` selector (gated by
  `respondsToSelector:` checks), so the webview renders at the display's native
  rate (ProMotion) instead of the ~60fps cap. Best-effort: an OS without those
  selectors stays at the default.

## Tiling under aerospace / yabai

A borderless window has no fullscreen button, so aerospace's dialog heuristic
floats it by default, and `ix-windows` has no bundle id to special-case. The
crate `README.md` gives the rules: an aerospace `on-window-detected` rule
matching `app-name-regex-substring = 'ix-windows'` with `run = 'layout tiling'`,
or `yabai -m rule --add app='^ix-windows$' manage=on`.

## Tests (`src/lib.rs:334`)

`shell` wraps the body in `#ix-root` and escapes the title;
`escape_text` covers `<`, `&`, `>`. The window/webview paths require a display
and are exercised manually.

[`HtmlView`]: ../dashboard-core/overview.md#wire-types-srcpanrs
[`WindowManager`]: #the-engine-windowmanager-srclibrs
[`WindowManager::new`]: #the-engine-windowmanager-srclibrs
[`apply_snapshot<T>(target, snapshot)`]: #the-engine-windowmanager-srclibrs
[`producer_gone(producer)`]: #the-engine-windowmanager-srclibrs
[`window_closed(window_id)`]: #the-engine-windowmanager-srclibrs
[`is_empty`]: #the-engine-windowmanager-srclibrs
