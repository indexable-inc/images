//! One guest toplevel = one `PaneWindow`: NSWindow + input view +
//! `CAMetalLayer` + persistent surface texture + `CAMetalDisplayLink`.
//!
//! Presentation pacing: the display link ticks at the panel's rate (up to
//! 120Hz on ProMotion) and hands us the drawable; we only encode/present when
//! a new guest frame (or a resize) made the window dirty, and the frame's
//! `seq` is acked right after the present is scheduled. The guest renders its
//! next frame off that ack, genlocking it to the display.

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AllocAnyThread, DefinedClass, MainThreadOnly, define_class};
use objc2_app_kit::{
    NSBackingStoreType, NSWindow, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSObjectProtocol, NSPoint, NSRect, NSRunLoop, NSSize,
    NSString,
};
use objc2_metal::MTLTexture;
use objc2_quartz_core::{
    CAFrameRateRange, CAMetalDisplayLink, CAMetalDisplayLinkDelegate, CAMetalDisplayLinkUpdate,
    CAMetalLayer,
};
use panes_protocol::{Encoding, Tile, WindowId};

use crate::app;
use crate::render::Renderer;

/// ProMotion range: let the system drop to 60 when we present nothing, chase
/// 120 when frames flow (Apple TN3178 / CAFrameRateRange docs: preferred
/// must sit inside [minimum, maximum]).
const FRAME_RATE_RANGE: CAFrameRateRange =
    CAFrameRateRange { minimum: 60.0, maximum: 120.0, preferred: 120.0 };

pub struct WindowParams {
    pub id: WindowId,
    pub title: String,
    pub app_id: String,
    pub width: u32,
    pub height: u32,
    pub scale: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct BufferSize {
    width: u32,
    height: u32,
}

/// Current window geometry in buffer pixels, sent in `ToGuest::Configure`.
pub struct SurfaceSize {
    pub width: u32,
    pub height: u32,
    pub scale: u32,
}

pub struct PaneWindow {
    pub id: WindowId,
    pub ns: Retained<NSWindow>,
    layer: Retained<CAMetalLayer>,
    link: Retained<CAMetalDisplayLink>,
    // The window and the display link both hold their delegates weakly
    // (AppKit convention); these fields are the strong references.
    _win_delegate: Retained<WinDelegate>,
    _link_delegate: Retained<LinkDelegate>,
    texture: Option<Retained<ProtocolObject<dyn MTLTexture>>>,
    buffer: BufferSize,
    /// Guest render scale from `WindowNew`, used to convert protocol pixel
    /// sizes to window points.
    guest_scale: u32,
    pending_ack: Option<u64>,
    dirty: bool,
    pub shown: bool,
    /// Set once `WindowGone` arrived; the next `windowShouldClose` says yes.
    pub closing: bool,
}

impl PaneWindow {
    pub fn new(
        mtm: MainThreadMarker,
        renderer: &Renderer,
        params: &WindowParams,
        title_prefix: &str,
    ) -> Self {
        let scale = f64::from(params.scale.max(1));
        let content = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(f64::from(params.width) / scale, f64::from(params.height) / scale),
        );
        let style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable
            | NSWindowStyleMask::Resizable;
        // SAFETY: standard initializer; `defer: false` so the window backing
        // exists immediately (the Metal layer needs a real backing scale).
        let ns = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                mtm.alloc(),
                content,
                style,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // SAFETY: `true` (the default for titled windows) would free the
        // ObjC object under our `Retained` on close.
        unsafe { ns.setReleasedWhenClosed(false) };
        ns.setTitle(&NSString::from_str(&format!("{title_prefix}{}", params.title)));
        ns.setAcceptsMouseMovedEvents(true);
        ns.center();

        let view = crate::view::PanesView::new(mtm, params.id, content);
        let layer = CAMetalLayer::new();
        layer.setDevice(Some(&renderer.device));
        layer.setPixelFormat(objc2_metal::MTLPixelFormat::BGRA8Unorm);
        // framebufferOnly: drawables are pure render targets, letting
        // CoreAnimation scan out the drawable directly (Apple, CAMetalLayer
        // docs); we never blit into or sample from them.
        layer.setFramebufferOnly(true);
        // displaySyncEnabled keeps presents vsynced; the ack loop, not a
        // free-running present queue, is our pacing mechanism.
        layer.setDisplaySyncEnabled(true);
        // 3 drawables (the default, pinned explicitly): 2 starves 120Hz when
        // CPU encode and scanout overlap, more only adds latency (Apple,
        // "Reduce Drawable Count" / maximumDrawableCount docs).
        layer.setMaximumDrawableCount(3);
        let backing = ns.backingScaleFactor();
        layer.setContentsScale(backing);
        layer.setDrawableSize(NSSize::new(
            content.size.width * backing,
            content.size.height * backing,
        ));
        view.setLayer(Some(&layer));
        view.setWantsLayer(true);
        ns.setContentView(Some(&view));
        let _ = ns.makeFirstResponder(Some(&view));

        eprintln!(
            "panes-host: window {} mapped: app_id={} {}x{}@{}",
            params.id, params.app_id, params.width, params.height, params.scale
        );

        let win_delegate = WinDelegate::new(mtm, params.id);
        ns.setDelegate(Some(ProtocolObject::from_ref(&*win_delegate)));

        let link_delegate = LinkDelegate::new(mtm, params.id);
        let link = CAMetalDisplayLink::initWithMetalLayer(CAMetalDisplayLink::alloc(), &layer);
        link.setDelegate(Some(ProtocolObject::from_ref(&*link_delegate)));
        link.setPreferredFrameRateRange(FRAME_RATE_RANGE);
        // Common modes include NSEventTrackingRunLoopMode, so ticks keep
        // coming during live resize (where presentsWithTransaction needs
        // per-tick redraws) and menu tracking.
        // SAFETY: the main run loop, and this code runs on the main thread.
        unsafe {
            link.addToRunLoop_forMode(
                &NSRunLoop::mainRunLoop(),
                objc2_foundation::NSRunLoopCommonModes,
            );
        }
        // Paused until the first frame arrives; ticking an empty window is
        // pure wasted power.
        link.setPaused(true);

        Self {
            id: params.id,
            ns,
            layer,
            link,
            _win_delegate: win_delegate,
            _link_delegate: link_delegate,
            texture: None,
            buffer: BufferSize { width: 0, height: 0 },
            guest_scale: params.scale.max(1),
            pending_ack: None,
            dirty: false,
            shown: false,
            closing: false,
        }
    }

    pub fn set_title(&self, title_prefix: &str, title: &str) {
        self.ns.setTitle(&NSString::from_str(&format!("{title_prefix}{title}")));
    }

    pub fn set_min_max(&self, min: Option<(u32, u32)>, max: Option<(u32, u32)>) {
        let scale = f64::from(self.guest_scale);
        if let Some((width, height)) = min {
            self.ns.setContentMinSize(NSSize::new(
                f64::from(width) / scale,
                f64::from(height) / scale,
            ));
        }
        if let Some((width, height)) = max {
            self.ns.setContentMaxSize(NSSize::new(
                f64::from(width) / scale,
                f64::from(height) / scale,
            ));
        }
    }

    /// Decode and upload a `WindowFrame` into the persistent texture. Always
    /// records `seq` for acking: a malformed tile must not stall the guest's
    /// frame loop, it is logged instead.
    pub fn apply_frame(
        &mut self,
        renderer: &Renderer,
        seq: u64,
        width: u32,
        height: u32,
        full: bool,
        tiles: &[Tile],
    ) {
        self.pending_ack = Some(seq);
        self.dirty = true;
        self.link.setPaused(false);
        if width == 0 || height == 0 {
            eprintln!("panes-host: window {}: zero-sized frame {seq}", self.id);
            return;
        }

        let size = BufferSize { width, height };
        let mut fresh_texture = false;
        if self.texture.is_none() || self.buffer != size {
            self.texture = renderer.make_texture(width, height);
            self.buffer = size;
            fresh_texture = true;
            if self.texture.is_none() {
                eprintln!("panes-host: window {}: texture alloc {width}x{height} failed", self.id);
                return;
            }
        }
        let Some(texture) = self.texture.as_ref() else {
            return;
        };

        // A full frame invalidates retained contents. Skip the clear when the
        // tiles already blanket the buffer (the common case: a resize frame
        // is one full-surface tile); tiles never overlap, so summed area is
        // coverage.
        let covered: u64 = tiles.iter().map(|tile| u64::from(tile.rect.w) * u64::from(tile.rect.h)).sum();
        if (full || fresh_texture) && covered < u64::from(width) * u64::from(height) {
            let zeros = vec![0u8; width as usize * height as usize * 4];
            Renderer::upload(
                texture,
                panes_protocol::Rect { x: 0, y: 0, w: width, h: height },
                &zeros,
            );
        }

        for tile in tiles {
            let rect = tile.rect;
            let in_bounds = rect.x.checked_add(rect.w).is_some_and(|right| right <= width)
                && rect.y.checked_add(rect.h).is_some_and(|bottom| bottom <= height);
            if !in_bounds || rect.w == 0 || rect.h == 0 {
                eprintln!("panes-host: window {}: tile out of bounds, skipped", self.id);
                continue;
            }
            let expected = rect.w as usize * rect.h as usize * 4;
            match tile.encoding {
                Encoding::Raw => {
                    if tile.payload.len() == expected {
                        Renderer::upload(texture, rect, &tile.payload);
                    } else {
                        eprintln!("panes-host: window {}: raw tile size mismatch", self.id);
                    }
                }
                Encoding::Lz4 => match lz4_flex::block::decompress(&tile.payload, expected) {
                    Ok(bytes) if bytes.len() == expected => Renderer::upload(texture, rect, &bytes),
                    Ok(_) => {
                        eprintln!("panes-host: window {}: lz4 tile size mismatch", self.id);
                    }
                    Err(error) => {
                        eprintln!("panes-host: window {}: lz4 decode failed: {error}", self.id);
                    }
                },
            }
        }
    }

    /// Present on a display-link tick if anything changed. Returns the seq to
    /// ack, which the caller sends only after the present was scheduled.
    pub fn present(
        &mut self,
        renderer: &Renderer,
        update: &CAMetalDisplayLinkUpdate,
    ) -> Option<u64> {
        if !self.dirty {
            return None;
        }
        let texture = self.texture.as_ref()?;
        let drawable = update.drawable();
        if renderer.draw(texture, &drawable, self.layer.presentsWithTransaction()) {
            self.dirty = false;
            self.pending_ack.take()
        } else {
            // Keep dirty + pending ack; retry next tick.
            None
        }
    }

    /// Redraw (stretching the stale texture) on the next tick; used during
    /// resize so the window never shows undefined drawable contents.
    pub fn mark_dirty(&mut self) {
        if self.texture.is_some() {
            self.dirty = true;
            self.link.setPaused(false);
        }
    }

    pub fn live_resize(&self, active: bool) {
        // During live resize presents ride the CATransaction so layer size
        // and contents change atomically with the window frame; outside it
        // the async path is faster and lower-latency.
        self.layer.setPresentsWithTransaction(active);
    }

    /// Sync layer geometry to the current view size, returning it for
    /// `Configure`.
    pub fn sync_layer_geometry(&self) -> SurfaceSize {
        let backing = self.ns.backingScaleFactor();
        let bounds = self.ns.contentView().map_or_else(
            || NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)),
            |view| view.bounds(),
        );
        self.layer.setContentsScale(backing);
        self.layer.setDrawableSize(NSSize::new(
            bounds.size.width * backing,
            bounds.size.height * backing,
        ));
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        SurfaceSize {
            width: (bounds.size.width * backing).round().max(0.0) as u32,
            height: (bounds.size.height * backing).round().max(0.0) as u32,
            scale: backing.round().max(1.0) as u32,
        }
    }

    /// Tear down outside any `APP` borrow: `close()` synchronously fires
    /// `windowWillClose` on the delegate, which re-enters app state.
    pub fn shutdown(self) {
        self.link.invalidate();
        self.ns.setDelegate(None);
        self.ns.close();
    }
}

struct DelegateIvars {
    id: WindowId,
}

define_class!(
    #[unsafe(super(objc2_foundation::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PanesWindowDelegate"]
    #[ivars = DelegateIvars]
    struct WinDelegate;

    unsafe impl NSObjectProtocol for WinDelegate {}

    unsafe impl NSWindowDelegate for WinDelegate {
        #[unsafe(method(windowShouldClose:))]
        fn window_should_close(&self, _sender: &NSWindow) -> bool {
            // Close is guest-driven: forward a CloseRequest and only really
            // close once WindowGone comes back (or the window is unknown).
            app::window_should_close(self.ivars().id)
        }

        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _notification: &NSNotification) {
            app::window_closed(self.ivars().id);
        }

        #[unsafe(method(windowDidResize:))]
        fn window_did_resize(&self, _notification: &NSNotification) {
            app::window_geometry_changed(self.ivars().id);
        }

        #[unsafe(method(windowDidChangeBackingProperties:))]
        fn window_did_change_backing_properties(&self, _notification: &NSNotification) {
            app::window_geometry_changed(self.ivars().id);
        }

        #[unsafe(method(windowWillStartLiveResize:))]
        fn window_will_start_live_resize(&self, _notification: &NSNotification) {
            app::window_live_resize(self.ivars().id, true);
        }

        #[unsafe(method(windowDidEndLiveResize:))]
        fn window_did_end_live_resize(&self, _notification: &NSNotification) {
            app::window_live_resize(self.ivars().id, false);
        }

        #[unsafe(method(windowDidBecomeKey:))]
        fn window_did_become_key(&self, _notification: &NSNotification) {
            app::window_activation(self.ivars().id, true);
        }

        #[unsafe(method(windowDidResignKey:))]
        fn window_did_resign_key(&self, _notification: &NSNotification) {
            app::window_activation(self.ivars().id, false);
        }
    }
);

impl WinDelegate {
    fn new(mtm: MainThreadMarker, id: WindowId) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DelegateIvars { id });
        unsafe { objc2::msg_send![super(this), init] }
    }
}

define_class!(
    #[unsafe(super(objc2_foundation::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PanesLinkDelegate"]
    #[ivars = DelegateIvars]
    struct LinkDelegate;

    unsafe impl NSObjectProtocol for LinkDelegate {}

    unsafe impl CAMetalDisplayLinkDelegate for LinkDelegate {
        #[unsafe(method(metalDisplayLink:needsUpdate:))]
        fn metal_display_link_needs_update(
            &self,
            _link: &CAMetalDisplayLink,
            update: &CAMetalDisplayLinkUpdate,
        ) {
            app::display_tick(self.ivars().id, update);
        }
    }
);

impl LinkDelegate {
    fn new(mtm: MainThreadMarker, id: WindowId) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DelegateIvars { id });
        unsafe { objc2::msg_send![super(this), init] }
    }
}
