# ix-windows

Render each live MCP resource as its own floating, blurred **overlay** webview
window that auto-sizes to its content.

`ix-windows` is a standalone consumer of the dashboard producer stream. The MCP
already publishes every resource onto the producer sockets as an `html` pane
keyed `resource/<id>` (see `packages/mcp`), so this process renders them with no
change to the MCP. A window opens when a resource appears, re-renders in place on
update, and closes when the resource closes or its producer disconnects.

```
nix run .#ix-windows            # watch the default discovery dir
nix run .#ix-windows -- --dir /tmp/ixw
```

## Overlay, not tiles

Each window is a chrome-less, always-on-top card floating above the desktop. No
tiling, no layout manager.

- **Blur behind.** The `wry` webview is transparent and is painted on top of a
  native `NSVisualEffectView` (behind-window blur), so the overlay frosts
  whatever is behind it. The content lives in a faintly tinted, rounded `#ix-root`
  panel for legibility; the blur layer is rounded and shadowed to match.
- **Auto-size to content.** There is no fixed window size. A `ResizeObserver` in
  the page measures the rendered panel and posts its pixel size over `wry`'s IPC
  channel; the OS window is grown or shrunk to fit (clamped to the monitor work
  area), so a window is exactly as big as the HTML it holds and expands as the
  content grows.
- **Floating across spaces.** The window is always-on-top and joins all spaces /
  floats over fullscreen apps (`NSWindowCollectionBehavior`).
- **Move, don't resize.** The card is borderless, so there is no title bar: drag
  it by its **chrome** (the padding/background around the content) to move it. The
  window is not user-resizable -- its size is owned by the content (auto-fit), so a
  manual resize would just fight the next content report.

## Self-contained HTML only

A resource's HTML is rendered inside a sandboxed, opaque-origin `<iframe>`
(`sandbox="allow-scripts"`, no `allow-same-origin`) loaded with no page origin, so
it must be **self-contained**: inline all CSS and JS and data. External CDN
scripts/styles, same-origin `fetch`, cookies, and storage are blocked by the
sandbox. Pre-render anything that needs a library and embed the result -- e.g.
render a mermaid diagram to SVG server-side (`kroki.io`, the `mermaid` CLI, ...)
and put the static `<svg>` in the HTML, rather than loading `mermaid.js` from a
CDN (which silently fails).

## macOS

- **120Hz.** WebKit's private experimental flag
  `PreferPageRenderingUpdatesNear60FPSEnabled` is disabled so the webview renders
  at the display's full refresh rate (ProMotion).
