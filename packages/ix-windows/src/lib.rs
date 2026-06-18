//! The reusable engine behind `ix-windows`: map a stream of dashboard
//! [`ProducerSnapshot`]s onto one floating, blurred **overlay** webview window
//! per live MCP resource.
//!
//! A [`WindowManager`] owns the open windows and reconciles them against each
//! snapshot: a new resource opens a window, a changed one re-renders in place
//! (no reload, so scroll and focus survive), a vanished one closes. It is
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
    /// Re-render in place if the resource's html or title changed. The body swap
    /// targets `#ix-root` so the document, its scroll position, and focus survive
    /// an update — a full reload would flicker and reset them. The page's
    /// `ResizeObserver` notices the resulting size change and posts a new size,
    /// which drives [`WindowManager::resize`].
    fn refresh(&mut self, pane: &Pane, html: &str) {
        if self.last_html != html {
            // `serde_json::to_string` emits a valid JS string literal (quoted and
            // escaped), so arbitrary resource HTML is injected safely.
            let literal = serde_json::to_string(html).unwrap_or_else(|_| "\"\"".to_owned());
            let js = format!("document.getElementById('ix-root').innerHTML = {literal};");
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
    /// Resources the user explicitly closed while still live. Without this, the
    /// next snapshot (any resource content change republishes one) would find
    /// the window gone and re-open it, fighting the user. Cleared when the
    /// resource actually vanishes or its producer disconnects, so a genuine
    /// re-registration opens a fresh window.
    dismissed: HashSet<PaneKey>,
    /// How many windows have been opened, used to cascade each new overlay so
    /// they do not stack exactly on top of each other.
    opened: u32,
}

impl WindowManager {
    /// An empty manager that emits resize events through `proxy`.
    #[must_use]
    pub fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        Self {
            proxy,
            windows: HashMap::new(),
            by_window: HashMap::new(),
            dismissed: HashSet::new(),
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
            } else if !self.dismissed.contains(&key) {
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

        // Forget dismissals for this producer's resources that are gone, so a
        // later re-registration of the same id opens a fresh window.
        self.dismissed
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
        self.dismissed.retain(|(p, _)| p != producer);
    }

    /// Forget a window the user closed (an OS `CloseRequested`) and remember the
    /// dismissal so a later snapshot for the still-live resource does not re-open
    /// it. Returns whether the window was one of ours.
    pub fn window_closed(&mut self, window: WindowId) -> bool {
        let Some(key) = self.by_window.remove(&window) else {
            return false;
        };
        self.windows.remove(&key);
        self.dismissed.insert(key);
        true
    }

    /// Fit the overlay window to the natural pixel size its content reported.
    /// Clamped to the window's monitor work area so an oversized resource grows
    /// scrollbars rather than spilling off-screen.
    ///
    /// The resize/reflow loop is broken primarily on the page side: `MEASURE_JS`
    /// only posts when the measured panel size actually changes, and the panel's
    /// intrinsic (`inline-block` / `max-width`) size does not depend on the
    /// window width. The 1px guard here only suppresses sub-pixel jitter and a
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
        // Borderless + transparent + always-on-top: a floating overlay card. The
        // macOS window server only rounds *titled* windows, so dropping
        // decorations leaves the corners to the blur view's own rounded layer.
        let builder = tao::window::WindowBuilder::new()
            .with_title(&pane.title)
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top(true)
            .with_inner_size(LogicalSize::new(INITIAL_SIZE.0, INITIAL_SIZE.1))
            .with_position(LogicalPosition::new(64.0 + step, 64.0 + step));
        let window = match builder.build(target) {
            Ok(window) => window,
            Err(error) => {
                eprintln!("ix-windows: window for {}: {error}", pane.id);
                // Record the key so a failing build is not retried on every
                // snapshot (which would churn OS windows for a live resource on a
                // host where window/webview creation persistently fails). Reuses
                // the dismissal lifecycle: cleared when the resource vanishes or
                // its producer disconnects, so a later environment can retry.
                self.dismissed.insert(key);
                return;
            }
        };
        let id = window.id();

        // The page measures its content and posts `"<w>x<h>"`; forward that as a
        // resize event tagged with this window so the loop can fit it.
        let proxy = self.proxy.clone();
        let webview = match WebViewBuilder::new()
            .with_transparent(true)
            .with_ipc_handler(move |request| {
                if let Some((w, h)) = parse_size(request.body().as_str()) {
                    let _ = proxy.send_event(UserEvent::Resize {
                        window: id,
                        width: w,
                        height: h,
                    });
                }
            })
            .with_html(shell(&pane.title, html))
            .build(&window)
        {
            Ok(webview) => webview,
            Err(error) => {
                eprintln!("ix-windows: webview for {}: {error}", pane.id);
                // As above: don't re-attempt a persistently failing build every
                // snapshot. The `window` local drops here, closing the OS window.
                self.dismissed.insert(key);
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

/// The chrome-less document a resource renders inside: a transparent shell whose
/// `#ix-root` panel holds the resource's own HTML (swapped in place on update)
/// and shrink-wraps it, plus a measuring script that posts the panel's pixel
/// size over `wry`'s IPC channel so the OS window can fit it.
fn shell(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
<title>{title}</title><style>{STYLE}</style></head>\
<body><div id=\"ix-root\">{body}</div><script>{MEASURE_JS}</script></body></html>",
        title = escape_text(title),
    )
}

/// Minimal escaping for text placed in an HTML text/attribute context (the
/// `<title>`). The body is producer-rendered HTML and is injected verbatim, the
/// same trust model as the web dashboard's sandboxed html pane.
fn escape_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Overlay shell styling: the document is fully transparent so the native blur
/// shows through; the content lives in `#ix-root`, a panel sized to its content's
/// intrinsic width (`width: max-content`) with a faint tint for legibility over
/// the blur and rounded corners matching the blur layer.
///
/// `width: max-content` (not plain `inline-block` shrink-to-fit) is load-bearing:
/// shrink-to-fit is capped at the containing block's width, i.e. the *current*
/// (initially tiny) window, so content wider than the window would wrap and the
/// window could never grow past its initial size. `max-content` measures the
/// content's true intrinsic width independent of the viewport; `max-width` caps
/// runaway width (the OS window resize is clamped to the monitor on top of that).
const STYLE: &str = "\
:root { color-scheme: dark; }
html, body { margin: 0; padding: 0; background: transparent; }
body {
  color: #cdd6f4;
  font: 14px/1.5 ui-monospace, 'SF Mono', Menlo, monospace;
}
#ix-root {
  display: inline-block;
  width: max-content;
  box-sizing: border-box;
  min-width: 120px;
  max-width: 1200px;
  padding: 16px 18px;
  background: rgba(30, 30, 46, 0.30);
  border-radius: 14px;
}
::-webkit-scrollbar { width: 10px; height: 10px; }
::-webkit-scrollbar-thumb { background: rgba(137, 140, 160, 0.5); border-radius: 5px; }
::-webkit-scrollbar-track { background: transparent; }
";

/// Measure the content panel and post its pixel size back to Rust whenever it
/// changes. `offsetWidth`/`offsetHeight` include the panel padding, so the OS
/// window ends up exactly as big as the rendered card. A `ResizeObserver` covers
/// both content swaps (the in-place `#ix-root` update) and late reflows (images,
/// fonts); reports are coalesced to one per frame and deduped, so a stable panel
/// posts once.
const MEASURE_JS: &str = "\
(function () {
  var root = document.getElementById('ix-root');
  if (!root) return;
  var lastW = -1, lastH = -1, pending = false;
  function report() {
    pending = false;
    var w = Math.ceil(root.offsetWidth);
    var h = Math.ceil(root.offsetHeight);
    if (w === lastW && h === lastH) return;
    lastW = w; lastH = h;
    if (window.ipc && window.ipc.postMessage) window.ipc.postMessage(w + 'x' + h);
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

    // SAFETY: ordinary Objective-C message sends on the main thread to freshly
    // created / live AppKit objects of the expected classes.
    unsafe {
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
        effect.setWantsLayer(true);
        if let Some(layer) = effect.layer() {
            layer.setCornerRadius(14.0);
            layer.setMasksToBounds(true);
        }
        // Place the blur beneath the webview (added as the content view's first
        // subview), so the rendered HTML paints on top of it.
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
    use super::{escape_text, parse_size, shell};

    #[test]
    fn shell_wraps_body_in_ix_root_and_escapes_title() {
        let out = shell("a <b> & c", "<p>hi</p>");
        assert!(out.contains("<div id=\"ix-root\"><p>hi</p></div>"));
        assert!(out.contains("<title>a &lt;b&gt; &amp; c</title>"));
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
