//! The reusable engine behind `ix-windows`: map a stream of dashboard
//! [`ProducerSnapshot`]s onto one floating, blurred **overlay** webview window
//! per live MCP resource.
//!
//! A [`WindowManager`] owns the open windows and reconciles them against each
//! snapshot: a new resource opens a window, a changed one re-renders (the
//! producer HTML lives in a sandboxed iframe whose `srcdoc` is swapped, so its
//! inner scroll resets), a vanished one closes. It is
//! deliberately decoupled from the event source for the window-creation target,
//! but it emits [`UserEvent::Resize`] back into the loop (so it needs the loop's
//! proxy), which fixes the loop's user-event type to [`UserEvent`].
//!
//! ## Overlay, not tiles
//!
//! Each window is a chrome-less, always-on-top card floating above the desktop:
//! a transparent `wry` webview painted on top of a native `NSVisualEffectView`
//! that blurs whatever is behind the window. There is no tiling and no layout
//! manager. Instead the window auto-sizes to its content: a `ResizeObserver` in
//! the page posts the rendered panel's pixel size over `wry`'s IPC channel, and
//! [`WindowManager::resize`] grows or shrinks the OS window to match (clamped to
//! the monitor), so a window is exactly as big as the HTML it holds.
//!
//! ## What counts as a resource
//!
//! The MCP publishes every `register_resource()` view (a terminal, a TUI screen,
//! a custom widget — all already rendered to HTML) as an [`HtmlView`] pane keyed
//! `resource/<id>` (see `packages/mcp/ix_notebook_mcp/pane_bridge.py`). This
//! engine windows exactly those panes; a producer's exec runs, namespace, and
//! cells stay on the web canvas.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use dashboard_core::{Pane, ProducerEvent, ProducerSnapshot, View};
use tao::dpi::{LogicalPosition, LogicalSize};
use tao::event_loop::{EventLoopProxy, EventLoopWindowTarget};
use tao::window::{Window, WindowId};
use wry::{WebView, WebViewBuilder};

/// Pane-id prefix marking an MCP resource. Mirrors the key built in
/// `pane_bridge.py` (`f"resource/{res['id']}"`).
const RESOURCE_PREFIX: &str = "resource/";

/// Logical size a freshly opened window starts at, before its content reports a
/// natural size and [`WindowManager::resize`] snaps it to fit.
const INITIAL_SIZE: (f64, f64) = (480.0, 300.0);

/// Events the binary's `tao` event loop carries. The subscriber thread feeds
/// [`UserEvent::Producer`]; the page's content-measuring script feeds
/// [`UserEvent::Resize`] back through `wry`'s IPC handler and the loop proxy.
#[derive(Debug, Clone)]
pub enum UserEvent {
    /// A producer-stream event (a new/updated snapshot or a gone producer).
    Producer(ProducerEvent),
    /// A window's content reported its natural pixel size; fit the OS window to
    /// it. `window` identifies the source webview's window.
    Resize {
        window: WindowId,
        width: f64,
        height: f64,
    },
    /// The user pressed on the card chrome (or Cmd-pressed anywhere); begin an
    /// interactive move of the window. A borderless, non-resizable window has no
    /// native title bar to drag, so `OUTER_JS` starts the drag and the loop calls
    /// `drag_window`.
    Drag { window: WindowId },
    /// The user clicked the floating close button; dismiss this window. A
    /// borderless window has no native close control, so the card paints its own
    /// `×` and posts `"close"`, which the loop routes to
    /// [`WindowManager::window_closed`].
    Close { window: WindowId },
}

/// A pane's global identity across producers: `(producer id, pane id)`. A pane id
/// is unique only within its producer, so the producer scopes it.
type PaneKey = (String, String);

/// One open resource window: its `tao` window, its `wry` webview, the last
/// content rendered into it (so an unchanged snapshot is a no-op), and the last
/// logical size applied (so a repeated resize report is a no-op).
struct OpenWindow {
    // Held to keep the OS window alive; dropping `OpenWindow` closes the window.
    window: Window,
    webview: WebView,
    last_html: String,
    last_title: String,
    last_size: (f64, f64),
}

impl OpenWindow {
    /// Re-render in place if the resource's html or title changed. The producer
    /// body lives inside a sandboxed `<iframe>`, so an update swaps the iframe's
    /// `srcdoc` (which reloads the iframe; scroll position inside it resets, the
    /// trade for never running producer script in the trusted document). The
    /// iframe's own measuring script reports the new size, which drives
    /// [`WindowManager::resize`].
    fn refresh(&mut self, pane: &Pane, html: &str) {
        if self.last_html != html {
            // Set `.srcdoc` to the sandboxed inner document. `serde_json::to_string`
            // emits a valid JS string literal, so the assignment is well-formed for
            // arbitrary producer HTML; the iframe sandbox (not this escaping) is
            // what contains any script in that HTML.
            let inner = serde_json::to_string(&inner_document(html))
                .unwrap_or_else(|_| "\"\"".to_owned());
            let js = format!("document.getElementById('ix-frame').srcdoc = {inner};");
            let _ = self.webview.evaluate_script(&js);
            html.clone_into(&mut self.last_html);
        }
        if self.last_title != pane.title {
            self.window.set_title(&pane.title);
            self.last_title.clone_from(&pane.title);
        }
    }
}

/// Owns the resource overlay windows and reconciles them against producer
/// snapshots. Emits [`UserEvent::Resize`] through the loop proxy, so it is tied
/// to a `tao` loop whose user-event type is [`UserEvent`].
pub struct WindowManager {
    proxy: EventLoopProxy<UserEvent>,
    windows: HashMap<PaneKey, OpenWindow>,
    /// Reverse index so an OS event (a close, a resize report) maps back to the
    /// pane it represents.
    by_window: HashMap<WindowId, PaneKey>,
    /// Session-only suppression keyed by `(producer, pane)`: a window/webview that
    /// failed to build (so we do not churn-retry it every snapshot). Cleared when
    /// the resource vanishes or its producer disconnects, so a later environment
    /// can retry. User dismissals live in `dismissed_keys` instead.
    failed: HashSet<PaneKey>,
    /// Resources the user explicitly closed, remembered **persistently** and keyed
    /// by `(producer, pane id)`, mirrored to disk ([`dismissed_file`]). The
    /// producer id is stable for the producing session's whole process lifetime, so
    /// this survives re-registration and an `ix-windows` restart, yet does **not**
    /// suppress a same-named pane from a *different* producer (e.g. two sessions
    /// each publishing `resource/queue`). Resets only when the producing session
    /// itself restarts (new producer id). Never auto-cleared; delete the file to
    /// forget all.
    dismissed_keys: HashSet<PaneKey>,
    /// How many windows have been opened, used to cascade each new overlay so
    /// they do not stack exactly on top of each other.
    opened: u32,
}

impl WindowManager {
    /// An empty manager that emits resize events through `proxy`.
    #[must_use]
    pub fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        let dismissed_keys = dismissed_file().map(|p| load_dismissed(&p)).unwrap_or_default();
        Self {
            proxy,
            windows: HashMap::new(),
            by_window: HashMap::new(),
            failed: HashSet::new(),
            dismissed_keys,
            opened: 0,
        }
    }

    /// Reconcile this producer's resource windows against its latest snapshot:
    /// open new resources, refresh changed ones, and close those that vanished.
    ///
    /// `target` is the running event loop, needed to create windows; it is
    /// generic over the loop's user-event type so the window-creation path stays
    /// independent of the binary's event enum.
    pub fn apply_snapshot<T: 'static>(
        &mut self,
        target: &EventLoopWindowTarget<T>,
        snapshot: &ProducerSnapshot,
    ) {
        let mut present: HashSet<&str> = HashSet::new();
        for pane in &snapshot.panes {
            let View::Html(view) = &pane.view else {
                continue;
            };
            if !pane.id.starts_with(RESOURCE_PREFIX) {
                continue;
            }
            present.insert(pane.id.as_str());
            let key = (snapshot.producer.clone(), pane.id.clone());
            if let Some(open) = self.windows.get_mut(&key) {
                open.refresh(pane, &view.html);
            } else if !self.failed.contains(&key) && !self.dismissed_keys.contains(&key) {
                self.open(target, key, pane, &view.html);
            }
        }

        // Close windows for resources this producer no longer reports.
        let stale: Vec<PaneKey> = self
            .windows
            .keys()
            .filter(|(producer, id)| {
                *producer == snapshot.producer && !present.contains(id.as_str())
            })
            .cloned()
            .collect();
        for key in stale {
            self.close(&key);
        }

        // Forget build-failure suppressions for this producer's resources that are
        // gone, so a later re-registration of the same id retries. (User
        // dismissals in `dismissed_keys` are intentionally permanent.)
        self.failed
            .retain(|(producer, id)| *producer != snapshot.producer || present.contains(id.as_str()));
    }

    /// Drop every window belonging to a producer that has disconnected.
    pub fn producer_gone(&mut self, producer: &str) {
        let gone: Vec<PaneKey> = self
            .windows
            .keys()
            .filter(|(p, _)| p == producer)
            .cloned()
            .collect();
        for key in gone {
            self.close(&key);
        }
        self.failed.retain(|(p, _)| p != producer);
    }

    /// Forget a window the user closed (an OS `CloseRequested`, or the card's own
    /// floating `×` via [`UserEvent::Close`]) and remember the dismissal
    /// **persistently** by resource id, so the resource stays closed across
    /// re-registration, producer reconnect, and restart (see `dismissed_keys`).
    /// Removing it from `windows` drops the `OpenWindow`, which closes the OS
    /// window. Returns whether the window was one of ours.
    pub fn window_closed(&mut self, window: WindowId) -> bool {
        let Some(key) = self.by_window.remove(&window) else {
            return false;
        };
        // Persist on first sighting of this key; the file is an append-only log, so
        // a no-op insert must not append a duplicate line.
        if self.dismissed_keys.insert(key.clone()) {
            if let Some(path) = dismissed_file() {
                append_dismissed(&path, &key);
            }
        }
        self.windows.remove(&key);
        true
    }

    /// Fit the overlay window to the natural pixel size its content reported.
    /// Clamped to the window's monitor work area so an oversized resource grows
    /// scrollbars rather than spilling off-screen.
    ///
    /// The resize/reflow loop is broken primarily on the page side: the iframe's
    /// `INNER_JS` only posts when its measured `#ix-content` size actually changes,
    /// and that panel's intrinsic (`width: max-content`) size does not depend on
    /// the window width. The 1px guard here only suppresses sub-pixel jitter and a
    /// repeated clamped-to-max report.
    pub fn resize(&mut self, window: WindowId, width: f64, height: f64) {
        let Some(key) = self.by_window.get(&window) else {
            return;
        };
        let Some(open) = self.windows.get_mut(key) else {
            return;
        };
        // Monitor geometry (origin + extent) in logical pixels, so the overlay
        // can be both sized to fit and kept fully on-screen.
        let monitor = open.window.current_monitor();
        let (origin, extent) = monitor.as_ref().map_or(
            (LogicalPosition::new(0.0, 0.0), LogicalSize::new(1600.0, 1000.0)),
            |m| {
                let s = m.scale_factor();
                (
                    m.position().to_logical::<f64>(s),
                    m.size().to_logical::<f64>(s),
                )
            },
        );
        // Leave breathing room so the overlay never butts the screen edge, and so
        // a fit always exists for the off-screen nudge below.
        let max_w = (extent.width * 0.92).max(120.0);
        let max_h = (extent.height * 0.92).max(80.0);
        let w = width.clamp(120.0, max_w);
        let h = height.clamp(80.0, max_h);
        if (w - open.last_size.0).abs() < 1.0 && (h - open.last_size.1).abs() < 1.0 {
            return;
        }
        open.last_size = (w, h);
        open.window.set_inner_size(LogicalSize::new(w, h));

        // The cascade offset (or a user-dragged position) plus the new size can
        // spill off the right/bottom edge; nudge the window back so it stays fully
        // visible. `w`/`h` are capped below the monitor extent, so `min <= max`
        // holds and a fit always exists.
        if let (Some(scale), Ok(pos)) = (
            monitor.as_ref().map(|m| m.scale_factor()),
            open.window.outer_position(),
        ) {
            let pos = pos.to_logical::<f64>(scale);
            // `.max(origin)` keeps `min <= max` even if a degenerate monitor is
            // narrower than the minimum window size (else `clamp` would panic).
            let nx = pos.x.clamp(origin.x, (origin.x + extent.width - w).max(origin.x));
            let ny = pos.y.clamp(origin.y, (origin.y + extent.height - h).max(origin.y));
            if (nx - pos.x).abs() >= 1.0 || (ny - pos.y).abs() >= 1.0 {
                open.window.set_outer_position(LogicalPosition::new(nx, ny));
            }
        }
    }

    /// Begin an interactive move of the window whose chrome the user pressed.
    /// `OUTER_JS` posts `"drag"` on mousedown over the card chrome; `drag_window`
    /// hands the rest of the gesture to the OS so the overlay tracks the cursor.
    /// A failure (e.g. no active press) is non-fatal -- the click is just ignored.
    pub fn begin_drag(&self, window: WindowId) {
        let Some(key) = self.by_window.get(&window) else {
            return;
        };
        if let Some(open) = self.windows.get(key) {
            let _ = open.window.drag_window();
        }
    }

    /// Whether any resource windows are currently open.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    /// Create a chrome-less, transparent, always-on-top overlay window for a
    /// resource pane, with a native blur behind its transparent webview.
    fn open<T: 'static>(
        &mut self,
        target: &EventLoopWindowTarget<T>,
        key: PaneKey,
        pane: &Pane,
        html: &str,
    ) {
        // Cascade each overlay so several do not perfectly overlap.
        let step = f64::from(self.opened % 8) * 32.0;
        self.opened = self.opened.wrapping_add(1);
        // Borderless + transparent + always-on-top: a floating overlay card. Not
        // user-resizable -- the window size is owned by the content (auto-fit via
        // `resize`), so a manual resize would just fight the next content report.
        // It stays movable: `OUTER_JS` starts a window drag on mousedown over the
        // card chrome (`UserEvent::Drag` -> `drag_window`), since a borderless,
        // non-resizable window has no native title bar to grab.
        // The overlay is a square card: a borderless window has square corners by
        // default and `install_blur` adds no layer rounding.
        let builder = tao::window::WindowBuilder::new()
            .with_title(&pane.title)
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top(true)
            .with_resizable(false)
            .with_inner_size(LogicalSize::new(INITIAL_SIZE.0, INITIAL_SIZE.1))
            .with_position(LogicalPosition::new(64.0 + step, 64.0 + step));
        let window = match builder.build(target) {
            Ok(window) => window,
            Err(error) => {
                eprintln!("ix-windows: window for {}: {error}", pane.id);
                // Record the key so a failing build is not retried on every
                // snapshot (which would churn OS windows for a live resource on a
                // host where window/webview creation persistently fails). `failed`
                // is cleared when the resource vanishes or its producer disconnects,
                // so a later environment can retry.
                self.failed.insert(key);
                return;
            }
        };
        let id = window.id();

        // A per-window secret the trusted outer document embeds in its close
        // message. Producer HTML runs in a sandboxed, opaque-origin iframe that
        // cannot read the outer document, so it can never learn this token --
        // making `close` unforgeable from producer script even on engines where
        // the injected `window.ipc` reaches subframes (e.g. WebView2). `drag`
        // stays intentionally relayable from the iframe (that is how Cmd-drag over
        // content works), but dismissing a window must be trusted-only.
        let token = close_token();
        let close_msg = format!("close:{token}");

        // The page measures its content and posts `"<w>x<h>"`; forward that as a
        // resize event tagged with this window so the loop can fit it.
        let proxy = self.proxy.clone();
        let webview = match WebViewBuilder::new()
            .with_transparent(true)
            .with_ipc_handler(move |request| {
                let body = request.body().as_str();
                if body == "drag" {
                    let _ = proxy.send_event(UserEvent::Drag { window: id });
                } else if body == close_msg {
                    let _ = proxy.send_event(UserEvent::Close { window: id });
                } else if let Some((w, h)) = parse_size(body) {
                    let _ = proxy.send_event(UserEvent::Resize {
                        window: id,
                        width: w,
                        height: h,
                    });
                }
            })
            .with_html(shell(&pane.title, html, &token))
            .build(&window)
        {
            Ok(webview) => webview,
            Err(error) => {
                eprintln!("ix-windows: webview for {}: {error}", pane.id);
                // As above: don't re-attempt a persistently failing build every
                // snapshot. The `window` local drops here, closing the OS window.
                self.failed.insert(key);
                return;
            }
        };

        // macOS native tuning: a blur behind the transparent webview, and the
        // 120Hz render-rate uncap. Both no-ops on an OS without the selectors.
        #[cfg(target_os = "macos")]
        {
            install_blur(&window);
            enable_high_refresh(&webview);
        }

        self.by_window.insert(id, key.clone());
        self.windows.insert(
            key,
            OpenWindow {
                window,
                webview,
                last_html: html.to_owned(),
                last_title: pane.title.clone(),
                last_size: INITIAL_SIZE,
            },
        );
    }

    /// Close one window and clear both indexes.
    fn close(&mut self, key: &PaneKey) {
        if let Some(open) = self.windows.remove(key) {
            self.by_window.remove(&open.window.id());
            // `open` drops here, closing the OS window.
        }
    }
}

/// Parse a `"<width>x<height>"` IPC body (the page's measured panel size) into a
/// pair of logical pixels. Returns `None` on anything malformed.
///
/// The body is attacker-controlled (any resource's verbatim HTML can call
/// `window.ipc.postMessage`), so non-finite or non-positive values are rejected
/// here rather than reaching [`WindowManager::resize`]: `f64::clamp` *panics* on
/// `NaN`, which would abort the whole event loop and kill every overlay.
fn parse_size(body: &str) -> Option<(f64, f64)> {
    let (w, h) = body.trim().split_once('x')?;
    let w: f64 = w.trim().parse().ok()?;
    let h: f64 = h.trim().parse().ok()?;
    if !w.is_finite() || !h.is_finite() || w <= 0.0 || h <= 0.0 {
        return None;
    }
    Some((w, h))
}

/// The trusted outer document a resource renders inside: a transparent `#ix-root`
/// panel (the tinted, rounded card on the blur) holding a **sandboxed** `<iframe>`
/// that contains the producer HTML. Producer markup and any script it carries run
/// only inside that opaque-origin sandbox (`sandbox="allow-scripts"`, no
/// `allow-same-origin`), exactly like the web dashboard's html pane
/// (`HtmlBody.svelte`): no access to this document, `window.ipc`, cookies,
/// storage, or local files. The outer script ([`OUTER_JS`]) only listens for the
/// iframe's own size message and forwards it to `wry`'s IPC channel.
///
/// The initial body rides in the iframe's `srcdoc` attribute (attribute-escaped);
/// updates swap the `.srcdoc` property (see [`OpenWindow::refresh`]).
///
/// `token` is this window's close secret: it is defined as a global only in this
/// trusted document (unreachable from the sandboxed iframe) and appended to the
/// `close` IPC message, so producer script cannot forge a dismissal. It is hex
/// (`close_token`), so it needs no escaping inside the JS string literal.
fn shell(title: &str, body: &str, token: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
<title>{title}</title><style>{STYLE}</style></head>\
<body><div id=\"ix-root\"><div id=\"ix-close\" title=\"Close\">\u{00d7}</div>\
<iframe id=\"ix-frame\" sandbox=\"allow-scripts\" \
srcdoc=\"{srcdoc}\"></iframe></div>\
<script>window.IX_CLOSE_TOKEN=\"{token}\";{OUTER_JS}</script></body></html>",
        title = escape_text(title),
        srcdoc = escape_attr(&inner_document(body)),
    )
}

/// An unguessable per-window token for the trusted `close` message: 128 bits of
/// OS randomness (`uuid` v4), rendered as 32 hex chars. Producer script is
/// untrusted and, on engines where the injected `window.ipc` reaches subframes,
/// could otherwise brute-force a predictable token; random bits remove that. It
/// lives only in the outer document, which the sandboxed opaque-origin iframe
/// cannot read, so the producer never sees it.
fn close_token() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Where user-dismissed resource ids are persisted across runs:
/// `$XDG_STATE_HOME/ix-windows/dismissed`, else `$HOME/.local/state/ix-windows/
/// dismissed`. `None` if neither env var is set (then dismissals are session-only).
/// Delete this file to forget every dismissal and let all resources open again.
fn dismissed_file() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(base.join("ix-windows").join("dismissed"))
}

/// Read the persisted dismissed keys. Each line is a JSON `[producer, id]` pair
/// (not a raw string): producer and id are agent-supplied and can contain anything
/// -- newlines, quotes, surrounding whitespace -- and JSON round-trips them
/// faithfully, where a raw-delimited format would split a multi-line value into
/// bogus entries (dropping the real dismissal and possibly suppressing an
/// unrelated key that matches a fragment). Blank lines are skipped and any line
/// that fails to parse is ignored (corruption tolerance). Any error
/// (missing/unreadable file) yields an empty set -- a lost log just reopens a few
/// windows, never a crash.
fn load_dismissed(path: &Path) -> HashSet<PaneKey> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return HashSet::new();
    };
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<PaneKey>(line.trim()).ok())
        .collect()
}

/// Append one dismissed key to the log as a JSON `[producer, id]` line (see
/// [`load_dismissed`] for why JSON, not raw text). Creates the parent dir and file
/// as needed. Best effort: a failure to persist only means the window may reopen on
/// the next run, so errors are swallowed rather than disrupting the overlay.
/// Callers must dedupe (only append a key not already in the in-memory set), since
/// this is an append-only log.
fn append_dismissed(path: &Path, key: &PaneKey) {
    use std::io::Write as _;
    let Ok(line) = serde_json::to_string(key) else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Owner-only: resource ids are agent-supplied and can embed private paths or
    // names, so the dismissal log must not be world-readable. `mode` covers a fresh
    // file; `set_permissions` tightens one created 0644 by an earlier build.
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    if let Ok(mut file) = options.open(path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
        }
        let _ = writeln!(file, "{line}");
    }
}

/// The sandboxed inner document for the iframe: the producer `body` verbatim in an
/// intrinsically-sized `#ix-content` panel, plus the measuring script
/// ([`INNER_JS`]) that posts its size out to the outer document. Loaded with an
/// opaque origin (the iframe sandbox), so the verbatim body is contained.
fn inner_document(body: &str) -> String {
    format!(
        "<!doctype html><meta charset=\"utf-8\"><style>{INNER_STYLE}</style>\
<div id=\"ix-content\">{body}</div><script>{INNER_JS}</script>"
    )
}

/// Minimal escaping for text in an HTML text context (the `<title>`).
fn escape_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Escape a string for a double-quoted HTML attribute value (the iframe
/// `srcdoc`). `&` and `"` can break out of a double-quoted value; `<`/`>` are also
/// escaped so producer markup never appears as live tags in the *outer* document
/// source. The browser decodes these references back before parsing `srcdoc` as
/// the iframe's document, so the inner document is reconstructed intact.
fn escape_attr(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Outer (trusted) document styling: fully transparent so the native blur shows
/// through; `#ix-root` is the tinted, rounded card that wraps the iframe edge to
/// edge (no padding) plus the floating `#ix-close` control. The iframe is
/// transparent and chrome-less; the outer script sizes it to the content size the
/// inner document reports, so `#ix-root` shrink-wraps it.
const STYLE: &str = "\
:root { color-scheme: dark; }
html, body { margin: 0; padding: 0; background: transparent; }
#ix-root {
  position: relative;
  display: inline-block;
  box-sizing: border-box;
  background: rgba(30, 30, 46, 0.20);
  /* Match the Rust-side window min clamp (120x80 in `resize`) so an empty or
     zero-height resource shows a small *visible* tinted card rather than a
     mostly-transparent, click-intercepting always-on-top window. */
  min-width: 120px;
  min-height: 80px;
  /* Square card (no border-radius). `overflow:hidden` still clips producer
     content to the card bounds. */
  overflow: hidden;
}
#ix-frame {
  display: block;
  border: 0;
  background: transparent;
  width: 120px;
  height: 80px;
}
/* Borderless windows have no native close control, so the card paints its own.
   It floats over the top-right corner at a faint base opacity, brightening on
   hover. A faint *always-on* base (not reveal-on-card-hover) is deliberate: the
   content fills the card via a sandboxed iframe, and hovering inside that opaque-
   origin iframe does not set `:hover` on the parent card, so a pure
   `#ix-root:hover` reveal would only ever show while the cursor sat on the button
   itself -- undiscoverable.
   `position: fixed` (not absolute) pins it to the webview viewport's top-right:
   when a resource is wider/taller than the monitor clamp the window shows scroll-
   bars over the oversized `#ix-root`, and an absolutely-positioned control would
   sit at the off-screen content edge. `#ix-root` sets no transform/filter, so the
   fixed control also escapes its `overflow:hidden` clip. */
#ix-close {
  position: fixed;
  top: 4px;
  right: 4px;
  z-index: 2;
  width: 16px;
  height: 16px;
  display: flex;
  align-items: center;
  justify-content: center;
  font: 13px/1 ui-monospace, 'SF Mono', Menlo, monospace;
  color: #cdd6f4;
  background: rgba(40, 40, 60, 0.55);
  border-radius: 50%;
  cursor: pointer;
  opacity: 0.4;
  transition: opacity 0.12s ease, background 0.12s ease;
  -webkit-user-select: none;
  user-select: none;
}
#ix-root:hover #ix-close { opacity: 0.75; }
#ix-close:hover { background: rgba(220, 80, 90, 0.9); opacity: 1; }
";

/// Inner (sandboxed) document styling: transparent so the outer card tint shows;
/// `#ix-content` shrink-wraps the producer body at its intrinsic width.
///
/// `width: max-content` (not plain `inline-block` shrink-to-fit) is load-bearing:
/// shrink-to-fit is capped at the containing block width, so content wider than
/// the iframe's current size would wrap and never grow it. `max-content` measures
/// the true intrinsic width; `max-width` caps runaway width (the OS window resize
/// is clamped to the monitor on top of that).
const INNER_STYLE: &str = "\
:root { color-scheme: dark; }
html, body { margin: 0; padding: 0; background: transparent; }
body {
  color: #cdd6f4;
  font: 14px/1.5 ui-monospace, 'SF Mono', Menlo, monospace;
}
#ix-content {
  display: inline-block;
  width: max-content;
  box-sizing: border-box;
  min-width: 120px;
  max-width: 1200px;
}
::-webkit-scrollbar { width: 4px; height: 4px; }
::-webkit-scrollbar-thumb { background: rgba(137, 140, 160, 0.5); border-radius: 2px; }
::-webkit-scrollbar-track { background: transparent; }
";

/// Runs in the trusted outer document. Listens for the iframe's size message,
/// sizes the iframe to it, then reports the card's pixel size to Rust over `wry`'s
/// IPC channel so the OS window can fit it. Only messages from our iframe with the
/// expected shape and finite positive numbers are honoured; everything else
/// (including any `postMessage` from producer script) is ignored.
const OUTER_JS: &str = "\
(function () {
  var frame = document.getElementById('ix-frame');
  var root = document.getElementById('ix-root');
  var close = document.getElementById('ix-close');
  if (!frame || !root) return;
  function ipc(msg) { if (window.ipc && window.ipc.postMessage) window.ipc.postMessage(msg); }
  // Drag the borderless window by any bare card chrome: a mousedown that reaches
  // this (outer, trusted) document landed outside the iframe (which captures its
  // own events). With zero padding the card shrink-wraps the content, so this
  // fires rarely; the iframe relays Cmd-drag and empty-background drags below.
  root.addEventListener('mousedown', function (event) {
    if (event.button !== 0 || event.target === close) return;
    ipc('drag');
  });
  if (close) {
    // Dismiss this window. The close message carries this window's secret token
    // (defined only in this trusted document, unreadable by the sandboxed iframe),
    // so producer script cannot forge a dismissal even if `window.ipc` reaches it.
    // No mousedown guard is needed: the root drag handler above already ignores
    // presses whose target is the close button.
    close.addEventListener('click', function (event) {
      event.stopPropagation();
      ipc('close:' + (window.IX_CLOSE_TOKEN || ''));
    });
  }
  window.addEventListener('message', function (event) {
    if (event.source !== frame.contentWindow) return;
    var data = event.data;
    // The iframe relays a Cmd-press (or a press on its empty background) so the
    // window stays movable even though content fills the card edge to edge.
    if (data && data.t === 'ixdrag') { ipc('drag'); return; }
    if (!data || data.t !== 'ixsize') return;
    var w = Number(data.w), h = Number(data.h);
    // Reject only garbage and negatives. Zero is a valid report (an empty resource,
    // or one that cleared its body): with zero padding we must still resize, or the
    // window would strand at its previous/initial size showing a blank, click-
    // blocking, always-on-top card. Floor the applied iframe size to 1px so the
    // measured card stays positive; Rust's `parse_size`/min-clamp then shrinks the
    // window to the minimum card (120x80) instead of dropping the report.
    if (!isFinite(w) || !isFinite(h) || w < 0 || h < 0) return;
    frame.style.width = Math.max(w, 1) + 'px';
    frame.style.height = Math.max(h, 1) + 'px';
    requestAnimationFrame(function () {
      // Measure with getBoundingClientRect (sub-pixel) and round UP, not
      // offsetWidth/Height (which round to the nearest integer, usually down): a
      // rounded-down report makes the OS window a fraction smaller than the card,
      // so the content overflows by < 1px and a scrollbar appears (and that
      // scrollbar then nudges the other axis into overflowing too). Ceiling the
      // true fractional size guarantees the window is >= the content, so neither
      // axis scrolls.
      var rect = root.getBoundingClientRect();
      var rw = Math.ceil(rect.width);
      var rh = Math.ceil(rect.height);
      ipc(rw + 'x' + rh);
    });
  });
})();
";

/// Runs inside the sandboxed iframe. Measures the producer content panel and posts
/// its pixel size to the parent whenever it changes (coalesced to one report per
/// frame, deduped), and relays a window-move gesture (Cmd+press anywhere, or a
/// press on the bare background) so the borderless window stays movable with no
/// card padding. It can only `postMessage` -- the sandbox denies it any access to
/// the parent document, `window.ipc`, cookies, storage, or local files.
const INNER_JS: &str = "\
(function () {
  var root = document.getElementById('ix-content');
  if (!root) return;
  // Keep the borderless window movable even though content fills it edge to edge:
  // a Cmd+press anywhere (works over content, without stealing clicks or text
  // selection) or a plain press on the bare background (no element under it)
  // relays a move gesture to the parent. The sandbox permits postMessage out but
  // grants no other access to the parent document.
  // Capture phase (the `true` below) so the gesture is seen even when producer
  // content (terminals, canvas widgets, draggable controls) calls
  // `stopPropagation()` on bubbling mousedowns -- otherwise such a card, having no
  // bare background, would be immovable.
  document.addEventListener('mousedown', function (event) {
    if (event.button !== 0) return;
    var bare = event.target === document.body || event.target === root;
    if (!event.metaKey && !bare) return;
    // Only suppress the default for a bare-background press, where it would start
    // a text selection. For a Cmd-press we deliberately do NOT preventDefault, so
    // an ordinary Cmd-click on producer content (e.g. open-in-... on a link) keeps
    // working: the relayed drag only actually moves the window if the pointer then
    // moves (a no-move Cmd-click round-trips to `drag_window` after mouseup, a
    // no-op), so click and window-move don't collide.
    if (bare && !event.metaKey) event.preventDefault();
    parent.postMessage({ t: 'ixdrag' }, '*');
  }, true);
  var lastW = -1, lastH = -1, pending = false;
  function report() {
    pending = false;
    // getBoundingClientRect + ceil (not offsetWidth/Height, which round to the
    // nearest integer and can land a pixel under the true fractional content
    // width): rounding up guarantees the iframe the outer doc sizes to this is
    // >= the content, so the producer pane never shows a sub-pixel scrollbar.
    var rect = root.getBoundingClientRect();
    var w = Math.ceil(rect.width);
    var h = Math.ceil(rect.height);
    if (w === lastW && h === lastH) return;
    lastW = w; lastH = h;
    parent.postMessage({ t: 'ixsize', w: w, h: h }, '*');
  }
  function schedule() {
    if (pending) return;
    pending = true;
    requestAnimationFrame(report);
  }
  new ResizeObserver(schedule).observe(root);
  window.addEventListener('load', schedule);
  schedule();
})();
";

/// Put a native `NSVisualEffectView` behind the (transparent) webview so the
/// window blurs whatever is behind it, and give the overlay a rounded, shadowed,
/// all-spaces-floating look.
///
/// Best-effort and main-thread-only: bails if the main-thread marker, the
/// `NSWindow` pointer, or its content view are unavailable.
#[cfg(target_os = "macos")]
fn install_blur(window: &Window) {
    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2_app_kit::{
        NSAutoresizingMaskOptions, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
        NSVisualEffectState, NSVisualEffectView, NSWindow, NSWindowCollectionBehavior,
        NSWindowOrderingMode,
    };
    use tao::platform::macos::WindowExtMacOS;

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    // SAFETY: on the main thread `tao` hands back a live, retained `NSWindow`
    // pointer; `Retained::retain` balances the +1 when this scope ends.
    let ns_window = unsafe { Retained::retain(window.ns_window().cast::<NSWindow>()) };
    let Some(ns_window): Option<Retained<NSWindow>> = ns_window else {
        return;
    };

    // These AppKit calls are safe in objc2 (the bindings encode their
    // thread/ownership requirements in the types), so no `unsafe` is needed; only
    // the raw `Retained::retain` above is.
    {
        let Some(content) = ns_window.contentView() else {
            return;
        };
        let frame = content.bounds();
        let effect = NSVisualEffectView::initWithFrame(mtm.alloc(), frame);
        effect.setMaterial(NSVisualEffectMaterial::HUDWindow);
        effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        effect.setState(NSVisualEffectState::Active);
        // The window opens at `INITIAL_SIZE` and then auto-grows via
        // `set_inner_size`. A flexible width+height mask keeps the blur filling
        // the content view as it resizes: an `NSWindow` always resizes its
        // content view to the content rect, and `contentView.autoresizesSubviews`
        // defaults on, so this tracks every resize with no explicit handler -
        // the same mechanism that keeps wry's own webview filling the window.
        effect.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        // Place the blur beneath the webview (added as the content view's first
        // subview), so the rendered HTML paints on top of it. A borderless window
        // is square by default, and we keep it square: no layer cornerRadius on
        // the blur or the content view.
        content.addSubview_positioned_relativeTo(&effect, NSWindowOrderingMode::Below, None);

        ns_window.setHasShadow(true);
        ns_window.invalidateShadow();
        // A true overlay: float over other spaces and over fullscreen apps.
        ns_window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
    }
}

/// Let the webview render at the display's native refresh rate (120Hz on
/// `ProMotion`) instead of `WebKit`'s default ~60fps cap, by disabling the
/// "Prefer Page Rendering Updates near 60fps" experimental feature.
///
/// That feature is not a `KVC` property (`setValue:forKey:` raises
/// `NSUnknownKeyException`), so it goes through the private
/// `_setEnabled:forExperimentalFeature:` API, gated by `respondsToSelector:`
/// checks. Best-effort: on an OS without these selectors the webview is simply
/// left at the default cap.
#[cfg(target_os = "macos")]
fn enable_high_refresh(webview: &WebView) {
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, Bool, NSObjectProtocol};
    use objc2::{ClassType, msg_send, sel};
    use objc2_foundation::NSString;
    use objc2_web_kit::WKPreferences;
    use wry::WebViewExtMacOS;

    /// The `WebKit` experimental-feature key for the ~60fps render cap.
    const FEATURE_KEY: &str = "PreferPageRenderingUpdatesNear60FPSEnabled";

    let wk = webview.webview();
    // SAFETY: ordinary Objective-C message sends to a live WKWebView and its
    // preferences on the main thread; the two private selectors are each gated by
    // a `responds_to` / `respondsToSelector:` check before use.
    unsafe {
        let prefs = wk.configuration().preferences();
        let class = WKPreferences::class();
        if !class.metaclass().responds_to(sel!(_experimentalFeatures))
            || !prefs.respondsToSelector(sel!(_setEnabled:forExperimentalFeature:))
        {
            return;
        }
        let features: Retained<AnyObject> = msg_send![class, _experimentalFeatures];
        let count: usize = msg_send![&*features, count];
        for i in 0..count {
            let feature: Retained<AnyObject> = msg_send![&*features, objectAtIndex: i];
            let key: Retained<NSString> = msg_send![&*feature, key];
            if key.to_string() == FEATURE_KEY {
                let _: () = msg_send![
                    &*prefs,
                    _setEnabled: Bool::new(false),
                    forExperimentalFeature: &*feature,
                ];
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        append_dismissed, close_token, escape_attr, escape_text, inner_document, load_dismissed,
        parse_size, shell,
    };

    #[test]
    fn dismissed_log_round_trips_keys_and_ignores_blanks() {
        // Unique temp path per run (close_token is a fresh uuid) so parallel tests
        // never collide and we never touch a real state dir.
        let dir = std::env::temp_dir().join(format!("ixw-dismiss-{}", close_token()));
        let path = dir.join("dismissed");
        assert!(load_dismissed(&path).is_empty(), "missing file -> empty set");

        // Faithful round-trip, including keys with embedded newlines, quotes, and
        // surrounding whitespace (a raw-line format would corrupt these), and the
        // same id under two different producers staying distinct.
        let keys: [(String, String); 4] = [
            ("46256-abc".to_owned(), "resource/docs/a.md".to_owned()),
            ("46256-abc".to_owned(), "resource/x\ny".to_owned()),
            ("99999-zzz".to_owned(), "resource/docs/a.md".to_owned()),
            ("p \"q".to_owned(), "resource/  spaced  ".to_owned()),
        ];
        for key in &keys {
            append_dismissed(&path, key);
        }
        let set = load_dismissed(&path);
        assert_eq!(set.len(), keys.len());
        for key in &keys {
            assert!(set.contains(key), "key not round-tripped: {key:?}");
        }

        // A stray blank line in the log must not become a phantom key.
        std::fs::write(&path, "[\"a\",\"resource/x\"]\n\n  \n[\"a\",\"resource/y\"]\n").unwrap();
        let set = load_dismissed(&path);
        assert_eq!(set.len(), 2);
        assert!(set.contains(&("a".to_owned(), "resource/x".to_owned())));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shell_sandboxes_body_in_an_iframe_and_escapes_title() {
        let out = shell("a <b> & c", "<p>hi</p>", "deadbeef");
        // Producer body lives in a sandboxed iframe, never in the trusted document.
        assert!(out.contains("<iframe id=\"ix-frame\" sandbox=\"allow-scripts\""));
        assert!(!out.contains("<div id=\"ix-root\"><p>hi</p>"));
        // The body rides the srcdoc attribute, attribute-escaped.
        assert!(out.contains("srcdoc=\""));
        assert!(out.contains("&lt;p&gt;hi&lt;/p&gt;") || out.contains("<p>hi</p>"));
        assert!(out.contains("<title>a &lt;b&gt; &amp; c</title>"));
    }

    #[test]
    fn shell_does_not_run_producer_script_in_the_trusted_document() {
        // A `<script>` in the body must end up inside the srcdoc attribute value
        // (escaped), not as a live top-level <script> in the outer document.
        let out = shell("t", "<script>steal()</script>", "deadbeef");
        assert!(!out.contains("<script>steal()</script>"));
        assert!(out.contains("&lt;script&gt;steal()&lt;/script&gt;"));
    }

    #[test]
    fn shell_embeds_close_token_in_the_trusted_document_only() {
        // The token defines the secret the close button appends; it lives in the
        // outer document (a top-level <script>), never inside the sandboxed iframe's
        // srcdoc, so the producer cannot read it to forge a dismissal.
        let out = shell("t", "<p>hi</p>", "deadbeef");
        assert!(out.contains("window.IX_CLOSE_TOKEN=\"deadbeef\""));
        // The srcdoc (everything the iframe sees) must not contain the token.
        let srcdoc = out.split("srcdoc=\"").nth(1).unwrap();
        let srcdoc = srcdoc.split("\"></iframe>").next().unwrap();
        assert!(!srcdoc.contains("deadbeef"));
    }

    #[test]
    fn close_token_is_128_bit_hex_and_unique() {
        let t = close_token();
        assert_eq!(t.len(), 32, "uuid v4 simple form is 32 hex chars");
        assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
        // Two tokens must differ (random, not a shared constant).
        assert_ne!(close_token(), close_token());
    }

    #[test]
    fn inner_document_holds_the_body_verbatim() {
        let inner = inner_document("<p>hi</p>");
        assert!(inner.contains("<div id=\"ix-content\"><p>hi</p></div>"));
    }

    #[test]
    fn escape_attr_covers_quote_and_amp() {
        assert_eq!(escape_attr(r#"a&b"c"#), "a&amp;b&quot;c");
    }

    #[test]
    fn escape_text_covers_markup_metachars() {
        assert_eq!(escape_text("<&>"), "&lt;&amp;&gt;");
    }

    #[test]
    fn parse_size_reads_width_x_height() {
        assert_eq!(parse_size("640x480"), Some((640.0, 480.0)));
        assert_eq!(parse_size(" 12.5 x 7 "), Some((12.5, 7.0)));
        assert_eq!(parse_size("nope"), None);
        assert_eq!(parse_size("640x"), None);
    }

    #[test]
    fn parse_size_rejects_non_finite_and_non_positive() {
        // These reach the parser straight from attacker-controlled page script.
        assert_eq!(parse_size("NaNx100"), None);
        assert_eq!(parse_size("infx100"), None);
        assert_eq!(parse_size("1e400x100"), None); // overflows to +inf
        assert_eq!(parse_size("-5x100"), None);
        assert_eq!(parse_size("0x0"), None);
    }
}
