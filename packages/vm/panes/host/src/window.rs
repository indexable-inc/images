//! One guest toplevel = one `PaneWindow`: `NSWindow` + input view +
//! `CAMetalLayer` + double-buffered surface textures + `CAMetalDisplayLink`.
//!
//! Presentation pacing: the display link ticks at the panel's rate (up to
//! 120Hz on `ProMotion`) and hands us the drawable; we only encode/present when
//! a new guest frame (or a resize) made the window dirty, and the frame's
//! `seq` is acked right after the present is scheduled. The guest renders its
//! next frame off that ack, genlocking it to the display. A fully occluded
//! window downshifts to slow ack-only ticks instead (see
//! [`PaneWindow::set_occluded`]): presents would be invisible, but a
//! withheld ack would wedge the guest.

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AllocAnyThread, DefinedClass, MainThreadOnly, define_class};
use objc2_app_kit::{
    NSBackingStoreType, NSView, NSWindow, NSWindowButton, NSWindowDelegate,
    NSWindowOcclusionState, NSWindowStyleMask, NSWindowTabbingMode, NSWindowTitleVisibility,
};
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSObjectProtocol, NSPoint, NSRect, NSRunLoop, NSSize,
    NSString,
};
use objc2_metal::{MTLCommandBuffer, MTLCommandBufferStatus, MTLDrawable, MTLTexture};
use objc2_quartz_core::{
    CAFrameRateRange, CAMetalDisplayLink, CAMetalDisplayLinkDelegate, CAMetalDisplayLinkUpdate,
    CAMetalLayer,
};
use panes_protocol::{Encoding, Rect, Tile, WindowId};

use crate::app;
use crate::render::Renderer;
use crate::view::PanesView;

/// The streaming tick range is pinned to a panel's max rate (min == max ==
/// preferred), not a 60..max span: with the adaptive range, a guest whose
/// ack-to-frame turnaround hovered near one period measured stretches of
/// downshifted ticks (probe guest, 2ms simulated render: ~99fps and p99 ack
/// RTT of 16ms adaptive vs ~116fps and 13.7ms pinned, index#1686). Ticks
/// only run while frames flow -- idle windows stop entirely via
/// [`IDLE_TICKS_TO_PAUSE`], which is where the power saving actually is --
/// so there is nothing for the adaptive range to win. Derived per window
/// from its own display (falling back to the main screen before placement)
/// and refreshed on `windowDidChangeScreen:`, so a window dragged between a
/// 60Hz external and the 120Hz panel chases whichever it is on.
fn stream_rate_range(screen: Option<&objc2_app_kit::NSScreen>) -> CAFrameRateRange {
    #[allow(clippy::cast_precision_loss)] // realistic refresh rates are tiny integers
    let max_fps = screen.map_or(60.0, |screen| screen.maximumFramesPerSecond() as f32);
    CAFrameRateRange { minimum: max_fps, maximum: max_fps, preferred: max_fps }
}

/// Tick rate while the window is fully occluded. The link keeps running as
/// the ack pacer, just slowly: pausing it and acking frames on receipt
/// instead was measured to unthrottle the guest to ~1700 frames/s (each ack
/// immediately releases the next frame), which burns far more CPU on both
/// sides than the presents it saves. ~10Hz keeps a covered window's stream
/// alive and cheap; occluded ticks skip the encode/present entirely.
const OCCLUDED_RATE_RANGE: CAFrameRateRange =
    CAFrameRateRange { minimum: 8.0, maximum: 12.0, preferred: 10.0 };

/// Ticks with nothing to present before the display link re-pauses (~250ms
/// at 120Hz). Pausing immediately after every present would put an unpause
/// round-trip inside the steady ack loop; a short idle grace keeps the
/// 120fps path hot while a quiet window stops ticking (and costing CPU)
/// shortly after its stream goes idle.
const IDLE_TICKS_TO_PAUSE: u32 = 30;

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

/// One decoded damage update, kept in [`Surface::log`] until every slot's
/// texture has absorbed it.
struct PendingTile {
    rect: Rect,
    bytes: Vec<u8>,
}

/// One of the window's two surface textures. Double-buffered because
/// `replaceRegion` does not synchronize against GPU access (Apple,
/// `MTLTexture` docs): a CPU upload into the texture a still-executing
/// command buffer is sampling for the previous present tears. Uploads only
/// go to a slot whose last draw has drained.
struct Slot {
    texture: Retained<ProtocolObject<dyn MTLTexture>>,
    /// Prefix of [`Surface::log`] already uploaded to this texture.
    absorbed: usize,
    /// The command buffer of the last present that sampled this texture;
    /// see the in-flight rule above.
    last_draw: Option<Retained<ProtocolObject<dyn MTLCommandBuffer>>>,
}

impl Slot {
    /// True while a committed draw sampling this texture may still be
    /// executing. `Error` counts as drained: the GPU dropped that work.
    fn in_flight(&self) -> bool {
        self.last_draw.as_ref().is_some_and(|commands| {
            !matches!(
                commands.status(),
                MTLCommandBufferStatus::Completed | MTLCommandBufferStatus::Error
            )
        })
    }
}

/// Double-buffered window surface: decoded damage is appended to `log` once
/// and replayed into a slot's texture right before that slot is drawn, so a
/// texture that sat out a present still catches up on the damage it missed.
struct Surface {
    slots: [Slot; 2],
    /// Slot the previous present drew; redraws (the live-resize stretch)
    /// sample it again, new content flips to the other slot.
    current: usize,
    log: Vec<PendingTile>,
    size: BufferSize,
}

impl Surface {
    fn new(renderer: &Renderer, size: BufferSize) -> Option<Self> {
        let slot = || {
            renderer
                .make_texture(size.width, size.height)
                .map(|texture| Slot { texture, absorbed: 0, last_draw: None })
        };
        Some(Self { slots: [slot()?, slot()?], current: 0, log: Vec::new(), size })
    }

    /// Append damage for both textures to absorb. A full-surface rect
    /// overwrites everything before it, so the backlog is dropped (this also
    /// re-bounds the log on every full/resize frame).
    fn push(&mut self, rect: Rect, bytes: Vec<u8>) {
        if rect.x == 0 && rect.y == 0 && rect.w == self.size.width && rect.h == self.size.height {
            self.log.clear();
            for slot in &mut self.slots {
                slot.absorbed = 0;
            }
        }
        self.log.push(PendingTile { rect, bytes });
    }

    /// Upload the pending log into every slot whose last draw has drained,
    /// then compact. Used on occluded ticks: no present absorbs the log
    /// there, and an animated window would grow it without bound while
    /// covered. Keeping the textures current also makes the un-occlusion
    /// redraw show the latest frame, not stale pixels.
    fn absorb_drained(&mut self) {
        for slot in &mut self.slots {
            if slot.absorbed < self.log.len() && !slot.in_flight() {
                for tile in &self.log[slot.absorbed..] {
                    Renderer::upload(&slot.texture, tile.rect, &tile.bytes);
                }
                slot.absorbed = self.log.len();
            }
        }
        self.compact();
    }

    /// Drop the log prefix every slot has absorbed. With one guest frame in
    /// flight and alternating presents this keeps the log a frame or two
    /// long.
    fn compact(&mut self) {
        let absorbed = self.slots[0].absorbed.min(self.slots[1].absorbed);
        if absorbed > 0 {
            self.log.drain(..absorbed);
            for slot in &mut self.slots {
                slot.absorbed -= absorbed;
            }
        }
    }
}

/// Current window geometry in buffer pixels, sent in `ToGuest::Configure`.
pub struct SurfaceSize {
    pub width: u32,
    pub height: u32,
    pub scale: u32,
}

// The bools are independent presentation/lifecycle flags, each owned by a
// different AppKit event path, not an encoded state machine.
#[allow(clippy::struct_excessive_bools)]
pub struct PaneWindow {
    pub id: WindowId,
    pub ns: Retained<NSWindow>,
    view: Retained<PanesView>,
    layer: Retained<CAMetalLayer>,
    link: Retained<CAMetalDisplayLink>,
    /// Tick range while frames flow (the window's panel max rate, pinned;
    /// see [`stream_rate_range`]); occlusion swaps it for
    /// [`OCCLUDED_RATE_RANGE`], re-expose restores it, and a display change
    /// recomputes it ([`PaneWindow::refresh_stream_rate`]).
    stream_rate: CAFrameRateRange,
    // The window and the display link both hold their delegates weakly
    // (AppKit convention); these fields are the strong references.
    _win_delegate: Retained<WinDelegate>,
    _link_delegate: Retained<LinkDelegate>,
    surface: Option<Surface>,
    /// Scale from `WindowNew`: the fixed unit for this window's protocol
    /// min/max sizes (protocol contract; the guest converts `WindowMinMax`
    /// at the same announced scale even if the client rescales later).
    guest_scale: u32,
    pending_ack: Option<u64>,
    dirty: bool,
    /// The window is fully covered (occlusion state lost `Visible`).
    /// `CAMetalDisplayLink` does not stop on its own when the window is
    /// occluded (measured: a covered `--mock` window keeps presenting and
    /// acking at the full 120Hz), so occlusion downshifts the link to
    /// [`OCCLUDED_RATE_RANGE`] and ticks stop presenting; see `present`.
    occluded: bool,
    /// Consecutive presentable ticks with nothing to draw; drives the idle
    /// re-pause (see [`IDLE_TICKS_TO_PAUSE`]).
    idle_ticks: u32,
    pub shown: bool,
    /// Set once `WindowGone` arrived; the next `windowShouldClose` says yes.
    pub closing: bool,
    /// The guest surface holds an active pointer lock
    /// (`ToHost::PointerLock`); the host engages the actual cursor capture
    /// only while this window is also key (see `app::sync_capture`), so the
    /// intent survives focus round-trips and re-engages on return.
    pub wants_lock: bool,
    /// Stock macOS chrome (`--native-titlebar`) instead of the default
    /// hidden-titlebar style; kept so title updates know whether to re-apply
    /// the hidden style (see [`PaneWindow::set_title`]).
    native_titlebar: bool,
}

impl PaneWindow {
    pub fn new(
        mtm: MainThreadMarker,
        renderer: &Renderer,
        params: &WindowParams,
        title_prefix: &str,
        native_titlebar: bool,
    ) -> Self {
        let scale = f64::from(params.scale.max(1));
        let content = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(f64::from(params.width) / scale, f64::from(params.height) / scale),
        );
        // `Titled` stays in the mask in both chrome modes: it is what gives
        // the window a normal frame and the standard accessibility window
        // role that tiling WMs (AeroSpace) manage; ghostty's hidden-titlebar
        // style keeps it for the same reason (HiddenTitlebarTerminalWindow.
        // swift: "We need `titled` in the mask to get the normal window
        // frame"). The default minimal chrome adds `FullSizeContentView` so
        // the content view (and the Metal layer sized off its bounds) spans
        // the full frame, including the strip the titlebar chrome would
        // have occupied.
        let mut style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable
            | NSWindowStyleMask::Resizable;
        if !native_titlebar {
            style |= NSWindowStyleMask::FullSizeContentView;
        }
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
        if !native_titlebar {
            apply_hidden_titlebar(&ns);
        }
        // Window placement belongs to the tiler (AeroSpace), and a draggable
        // background would turn guest-bound clicks into window moves in
        // floating mode; ghostty likewise leaves this off for terminal
        // windows. Floating windows still move via option-drag on the frame
        // edges (standard macOS) or the tiler's move commands.
        ns.setMovableByWindowBackground(false);
        ns.setAcceptsMouseMovedEvents(true);
        ns.center();

        let view = PanesView::new(mtm, params.id, content);
        let layer = CAMetalLayer::new();
        layer.setDevice(Some(&renderer.device));
        layer.setPixelFormat(objc2_metal::MTLPixelFormat::BGRA8Unorm);
        // framebufferOnly: drawables are pure render targets, letting
        // CoreAnimation scan out the drawable directly (Apple, CAMetalLayer
        // docs); we never blit into or sample from them.
        layer.setFramebufferOnly(true);
        // Present immediately instead of queueing for vsync. Measured with
        // the latency probe (index#1686): synced presents through the
        // windowed-compositing path hit glass a constant ~40.6ms after the
        // display-link tick -- ~5 frames, immovable by maximumDrawableCount
        // or preferredFrameLatency -- while immediate presents measure
        // 23.7-32.1ms (`MTLDrawable.presentedTime` ground truth), the
        // WindowServer sampling floor, at an unchanged 120fps. There is no
        // tearing exposure on macOS: WindowServer still composites whole
        // surfaces; and no free-running present loop either, because
        // presents stay display-link-tick-paced (the ack loop is the
        // throttle), so "sync off" only stops presents from queueing behind
        // extra vsyncs.
        layer.setDisplaySyncEnabled(false);
        // 2 drawables (the documented minimum), not the default 3: the third
        // only buys headroom when CPU encode approaches a full frame, and
        // ours is one fullscreen triangle. Measured either way at a steady
        // 120fps with the probe guest (index#1686) -- with immediate
        // presents (displaySyncEnabled false above) the count does not touch
        // glass latency either -- so the smaller pool just saves one
        // window-sized surface.
        layer.setMaximumDrawableCount(2);
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
        // Before the window is ordered in, `screen()` is None; the main
        // screen is where `center()` will place it.
        let stream_rate =
            stream_rate_range(ns.screen().or_else(|| objc2_app_kit::NSScreen::mainScreen(mtm)).as_deref());
        link.setPreferredFrameRateRange(stream_rate);
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
            view,
            layer,
            link,
            stream_rate,
            _win_delegate: win_delegate,
            _link_delegate: link_delegate,
            surface: None,
            guest_scale: params.scale.max(1),
            pending_ack: None,
            dirty: false,
            occluded: false,
            idle_ticks: 0,
            shown: false,
            closing: false,
            wants_lock: false,
            native_titlebar,
        }
    }

    /// The input view, for calls that must run outside the `APP` borrow
    /// (they send protocol messages, which re-enters app state).
    pub fn view_handle(&self) -> Retained<PanesView> {
        self.view.clone()
    }

    /// Re-pin the streaming tick range to the display the window is now on
    /// (`windowDidChangeScreen:`); applied immediately unless occluded (the
    /// occlusion path restores `stream_rate` itself on re-expose).
    pub fn refresh_stream_rate(&mut self, mtm: MainThreadMarker) {
        self.stream_rate = stream_rate_range(
            self.ns.screen().or_else(|| objc2_app_kit::NSScreen::mainScreen(mtm)).as_deref(),
        );
        if !self.occluded {
            self.link.setPreferredFrameRateRange(self.stream_rate);
        }
    }

    /// Trace-only: what the layer currently claims as its drawable cap
    /// (`CAMetalDisplayLink` may manage the pool behind our back).
    pub fn max_drawable_count(&self) -> usize {
        self.layer.maximumDrawableCount()
    }

    pub fn set_title(&self, title_prefix: &str, title: &str) {
        self.ns.setTitle(&NSString::from_str(&format!("{title_prefix}{title}")));
        // Setting the title re-reveals the native title view on macOS 15+;
        // ghostty re-applies the hidden style from its `title` override for
        // exactly this reason (HiddenTitlebarTerminalWindow.swift).
        if !self.native_titlebar {
            apply_hidden_titlebar(&self.ns);
        }
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

    /// Decode a `WindowFrame` into the surface damage log; the upload into
    /// whichever texture is safe to write happens on the presenting tick.
    /// Returns true when the frame will be presented (its ack rides the next
    /// display-link tick). Returns false when the host cannot take the frame
    /// at all (zero size, texture allocation failure): the caller must ack
    /// `seq` immediately, because with one-frame-in-flight guest pacing a
    /// withheld ack wedges that window's frame loop forever. Malformed tiles
    /// inside an otherwise valid frame are logged and skipped, never stall.
    pub fn apply_frame(
        &mut self,
        renderer: &Renderer,
        seq: u64,
        width: u32,
        height: u32,
        full: bool,
        tiles: Vec<Tile>,
    ) -> bool {
        // Metal's max texture dimension on Apple GPUs; also bounds what a
        // corrupted stream can demand (the zero-fill below allocates
        // width*height*4 host bytes, so unchecked wire dims could OOM).
        const MAX_DIM: u32 = 16_384;
        let size = BufferSize { width, height };
        let mut fresh_surface = false;
        let unpresentable = if width == 0 || height == 0 {
            eprintln!("panes-host: window {}: zero-sized frame {seq}", self.id);
            true
        } else if width > MAX_DIM || height > MAX_DIM {
            eprintln!("panes-host: window {}: {width}x{height} frame exceeds max dim", self.id);
            true
        } else if self.surface.as_ref().is_none_or(|surface| surface.size != size) {
            self.surface = Surface::new(renderer, size);
            fresh_surface = true;
            if self.surface.is_none() {
                eprintln!("panes-host: window {}: texture alloc {width}x{height} failed", self.id);
            }
            self.surface.is_none()
        } else {
            false
        };
        if unpresentable {
            // Nothing drawable: drop any older pending ack too (the caller's
            // immediate ack of the newer `seq` supersedes it; acks are
            // cumulative "presented up to").
            self.pending_ack = None;
            self.dirty = false;
            return false;
        }
        let Some(surface) = self.surface.as_mut() else {
            unreachable!("unpresentable path returned above");
        };

        // A frame that mismatches the drawable presents scaled (the render
        // pass samples, it never crops), which must never happen silently.
        // Logged once per settled buffer size (fresh surface, outside live
        // resize where per-tick mismatch is the norm) and worded as a state
        // note, not an error: one line per window is the EXPECTED startup
        // transition when a client that mapped at 1x re-renders 2x after the
        // host's scale reaches it. Only a persistent repeat (every resize
        // settles mismatched) means a scale-blind client rendering soft.
        if fresh_surface && !self.view.inLiveResize() {
            let drawable = self.layer.drawableSize();
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let (dw, dh) =
                (drawable.width.round().max(0.0) as u32, drawable.height.round().max(0.0) as u32);
            if (width, height) != (dw, dh) {
                eprintln!(
                    "panes-host: window {}: presenting {width}x{height} frames scaled onto the \
                     {dw}x{dh} drawable (brief while the guest adopts a scale change; persistent \
                     only for a client stuck at another buffer scale)",
                    self.id
                );
            }
        }

        let in_bounds = |rect: Rect| {
            rect.w > 0
                && rect.h > 0
                && rect.x.checked_add(rect.w).is_some_and(|right| right <= width)
                && rect.y.checked_add(rect.h).is_some_and(|bottom| bottom <= height)
        };
        // A full frame invalidates retained contents. Skip the clear only
        // when the accepted tiles already blanket the buffer (the common
        // case: a resize frame is one full-surface tile); tiles never
        // overlap, so summed area is coverage. Rejected tiles must not
        // count, or an out-of-bounds tile could skip the clear while leaving
        // pixels unwritten.
        let covered: u64 = tiles
            .iter()
            .filter(|tile| in_bounds(tile.rect))
            .map(|tile| u64::from(tile.rect.w) * u64::from(tile.rect.h))
            .sum();
        if (full || fresh_surface) && covered < u64::from(width) * u64::from(height) {
            let zeros = vec![0u8; width as usize * height as usize * 4];
            surface.push(Rect { x: 0, y: 0, w: width, h: height }, zeros);
        }

        for tile in tiles {
            let rect = tile.rect;
            if !in_bounds(rect) {
                eprintln!("panes-host: window {}: tile out of bounds, skipped", self.id);
                continue;
            }
            let expected = rect.w as usize * rect.h as usize * 4;
            match tile.encoding {
                Encoding::Raw => {
                    if tile.payload.len() == expected {
                        surface.push(rect, tile.payload);
                    } else {
                        eprintln!("panes-host: window {}: raw tile size mismatch", self.id);
                    }
                }
                Encoding::Lz4 => match lz4_flex::block::decompress(&tile.payload, expected) {
                    Ok(bytes) if bytes.len() == expected => surface.push(rect, bytes),
                    Ok(_) => {
                        eprintln!("panes-host: window {}: lz4 tile size mismatch", self.id);
                    }
                    Err(error) => {
                        eprintln!("panes-host: window {}: lz4 decode failed: {error}", self.id);
                    }
                },
            }
        }

        self.pending_ack = Some(seq);
        self.dirty = true;
        self.idle_ticks = 0;
        self.link.setPaused(false);
        true
    }

    /// Occlusion change from the window delegate. The link must never be
    /// paused here outright: with one-frame-in-flight guest pacing the tick
    /// is what releases the pending ack, and a withheld ack wedges the guest
    /// window (its compositor watchdog then resends full frames forever). So
    /// occlusion only downshifts the tick rate; the occluded branch of
    /// `present` turns those ticks into ack-only ticks.
    pub fn set_occluded(&mut self, occluded: bool) {
        self.occluded = occluded;
        if occluded {
            self.link.setPreferredFrameRateRange(OCCLUDED_RATE_RANGE);
        } else {
            self.link.setPreferredFrameRateRange(self.stream_rate);
            // Re-expose shows the freshest guest frame: occluded ticks kept
            // the textures current but never drew them.
            self.mark_dirty();
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
            // Quiet stream: stop ticking after a short grace so an idle
            // window costs nothing; the next frame/resize unpauses.
            self.idle_ticks = self.idle_ticks.saturating_add(1);
            if self.idle_ticks >= IDLE_TICKS_TO_PAUSE {
                self.link.setPaused(true);
            }
            return None;
        }
        let surface = self.surface.as_mut()?;
        if self.occluded {
            // Ack-only tick for a covered window: keep the textures current
            // (re-expose must show the latest frame) and release the ack so
            // the guest stays paced, but never encode/present invisible
            // pixels. The downshifted tick rate (see OCCLUDED_RATE_RANGE) is
            // what throttles the stream while covered.
            surface.absorb_drained();
            self.dirty = false;
            self.idle_ticks = 0;
            return self.pending_ack.take();
        }
        // Pick the texture to draw. A caught-up current slot means nothing
        // new arrived (a pure redraw, e.g. the live-resize stretch): sample
        // it again, no CPU write, no race. Otherwise flip to the other slot
        // so the pending uploads never touch the texture the previous
        // present may still be reading (`replaceRegion` does not
        // synchronize against the GPU; writing the in-flight texture is the
        // tearing race this double buffer exists to prevent).
        let index = if surface.slots[surface.current].absorbed == surface.log.len() {
            surface.current
        } else {
            surface.current ^ 1
        };
        let slot = &mut surface.slots[index];
        if slot.absorbed != surface.log.len() {
            if slot.in_flight() {
                // GPU still reading the write target (two presents behind):
                // keep dirty + pending ack and retry next tick.
                return None;
            }
            for tile in &surface.log[slot.absorbed..] {
                Renderer::upload(&slot.texture, tile.rect, &tile.bytes);
            }
            slot.absorbed = surface.log.len();
        }
        let drawable = update.drawable();
        if crate::trace::enabled()
            && let Some(seq) = self.pending_ack
        {
            // Ground truth for tick-to-glass latency: presentedTime is the
            // host time the drawable actually hit the screen, reported by
            // Metal after the fact (registered before the present below, as
            // the API requires). Trace-only; the block costs nothing when
            // tracing is off.
            let block = block2::RcBlock::new(
                move |presented: core::ptr::NonNull<ProtocolObject<dyn MTLDrawable>>| {
                    // SAFETY: Metal passes the presented drawable, valid for
                    // the duration of the handler.
                    let time = unsafe { presented.as_ref() }.presentedTime();
                    eprintln!("panes-trace glass seq={seq} presented={time:.6}");
                },
            );
            // SAFETY: as_ptr yields a valid block pointer; Metal copies the
            // block, so it need not outlive this call.
            unsafe { drawable.addPresentedHandler(block2::RcBlock::as_ptr(&block)) };
        }
        let Some(commands) =
            renderer.draw(&slot.texture, &drawable, self.layer.presentsWithTransaction())
        else {
            // Keep dirty + pending ack, but keep the slot switch: its
            // texture already absorbed the damage, so the retry next tick
            // just redraws it.
            surface.current = index;
            return None;
        };
        slot.last_draw = Some(commands);
        surface.current = index;
        surface.compact();
        self.dirty = false;
        self.idle_ticks = 0;
        self.pending_ack.take()
    }

    /// True while any part of the window is on screen (`AppKit` occlusion
    /// state contains `Visible`).
    pub fn occlusion_visible(&self) -> bool {
        self.ns.occlusionState().contains(NSWindowOcclusionState::Visible)
    }

    /// Redraw (stretching the stale texture) on the next tick; used during
    /// resize so the window never shows undefined drawable contents.
    pub fn mark_dirty(&mut self) {
        if self.surface.is_some() {
            self.dirty = true;
            self.idle_ticks = 0;
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

/// Ghostty's `macos-titlebar-style = hidden` recipe
/// (`HiddenTitlebarTerminalWindow.swift`), the default chrome: the window
/// keeps its `Titled` frame but every piece of titlebar chrome goes away, so
/// the guest surface fills a flat, edge-to-edge rectangle. Ghostty keeps the
/// native shadow and rounded corners in this style deliberately: truly
/// square corners require removing `Titled` (its `window-decoration = none`
/// path), which downgrades the accessibility role and stops tiling WMs from
/// managing the window; ghostty's own config docs steer users to the hidden
/// style instead. Idempotent; re-applied after every title change.
fn apply_hidden_titlebar(ns: &NSWindow) {
    // The title string stays set (Mission Control, the app switcher, and
    // AeroSpace's window list read it); only its rendering is hidden.
    ns.setTitleVisibility(NSWindowTitleVisibility::Hidden);
    ns.setTitlebarAppearsTransparent(true);
    for kind in
        [NSWindowButton::CloseButton, NSWindowButton::MiniaturizeButton, NSWindowButton::ZoomButton]
    {
        if let Some(button) = ns.standardWindowButton(kind) {
            button.setHidden(true);
        }
    }
    // No titlebar means nowhere to render a native tab bar (ghostty
    // disallows tabbing in its hidden style for the same reason).
    ns.setTabbingMode(NSWindowTabbingMode::Disallowed);
    // Even transparent, NSTitlebarContainerView sits above the content view
    // and turns clicks in the top strip into a window drag instead of guest
    // input. Ghostty hides the container outright ("nuke it from orbit");
    // same here, so the full frame delivers events to the guest.
    // SAFETY: superview is a plain accessor; main thread only.
    if let Some(frame) = ns.contentView().and_then(|view| unsafe { view.superview() }) {
        hide_titlebar_container(&frame);
    }
}

/// Depth-first search for the private `NSTitlebarContainerView` (a child of
/// the theme frame; the walk is recursive only as cheap insurance against
/// `AppKit` reshuffling the hierarchy). Matching on the class name is the same
/// unavoidable private-API touch ghostty ships.
fn hide_titlebar_container(view: &NSView) -> bool {
    for subview in view.subviews() {
        if subview.class().name().to_bytes() == b"NSTitlebarContainerView" {
            subview.setHidden(true);
            return true;
        }
        if hide_titlebar_container(&subview) {
            return true;
        }
    }
    false
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

        #[unsafe(method(windowDidChangeOcclusionState:))]
        fn window_did_change_occlusion_state(&self, _notification: &NSNotification) {
            app::window_occlusion_changed(self.ivars().id);
        }

        #[unsafe(method(windowDidChangeScreen:))]
        fn window_did_change_screen(&self, _notification: &NSNotification) {
            // The pinned tick range chases the display the window is on.
            app::window_screen_changed(self.ivars().id);
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
