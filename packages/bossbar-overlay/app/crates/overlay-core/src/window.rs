//! Float-window mechanics shared by every overlay: a transparent, borderless,
//! always-on-top window with no Dock presence, plus the surface and adapter
//! plumbing and a non-activating raise. The desktop stays click-through wherever
//! no overlay window sits, because there is simply no window there to intercept
//! the pointer.

use winit::dpi::{LogicalPosition, PhysicalPosition, PhysicalSize};
use winit::event_loop::EventLoop;
use winit::window::{Window, WindowAttributes, WindowLevel};

/// Move `window` to `new_pos` (logical points) and warp the pointer so it stays
/// glued to the same spot on the window.
///
/// A two-finger scroll-drag moves the window ourselves (unlike a press-drag, where
/// `Window::drag_window` hands the OS a drag loop that carries the pointer along).
/// Without warping, the window slides out from under a stationary pointer.
/// `cursor` is the pointer's last position relative to the window in physical
/// pixels (e.g. [`crate::DragClick::cursor`]); placing it at the same spot on the
/// moved window drags the pointer along. `None` (pointer position unknown) just
/// moves the window.
///
/// On macOS we do the warp ourselves rather than via `Window::set_cursor_position`
/// because that recomputes the target from a window-origin read-back, which lags
/// behind during a fast scroll and leaves the pointer drifting further behind each
/// move until it falls off the overlay (and scroll events stop reaching it). We
/// instead anchor the warp to `new_pos`, the position we just set, so the pointer
/// lands exactly on the moved window every tick. See `warp_cursor`.
pub fn move_window_with_cursor(
    window: &Window,
    new_pos: LogicalPosition<f64>,
    cursor: Option<PhysicalPosition<f64>>,
) {
    window.set_outer_position(new_pos);
    let Some(c) = cursor else {
        return;
    };
    #[cfg(target_os = "macos")]
    {
        // Global display point = window top-left (logical) + cursor offset (logical).
        // A borderless overlay's outer and content origins coincide, and winit's
        // logical screen space matches CoreGraphics' top-left global space, so this
        // is the same mapping `set_cursor_position` uses, but anchored to the value
        // we just set instead of a lagging read-back.
        let sf = window.scale_factor();
        warp_cursor(new_pos.x + c.x / sf, new_pos.y + c.y / sf);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window.set_cursor_position(c);
    }
}

/// Warp the pointer to global display point `(x, y)` (top-left origin, points).
///
/// `CGWarpMouseCursorPosition` moves the cursor without posting an event; the
/// follow-up `CGAssociateMouseAndMouseCursorPosition(true)` re-links mouse and
/// cursor so the warp does not leave hardware pointer movement briefly suppressed
/// (the default post-warp behavior), keeping a rapid scroll-drag smooth.
#[cfg(target_os = "macos")]
fn warp_cursor(x: f64, y: f64) {
    #[repr(C)]
    struct CGPoint {
        x: f64,
        y: f64,
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGWarpMouseCursorPosition(new_cursor_position: CGPoint) -> i32;
        fn CGAssociateMouseAndMouseCursorPosition(connected: i32) -> i32;
    }

    // SAFETY: `CGPoint` is a POD pair of `f64`; both calls take scalar args and run
    // on the winit main thread. Return codes are best-effort and ignored.
    unsafe {
        CGWarpMouseCursorPosition(CGPoint { x, y });
        CGAssociateMouseAndMouseCursorPosition(1);
    }
}

/// The pointer's current global display location (top-left origin, points), or
/// `None` if it cannot be read. Reads the *real* cursor (whatever last moved it,
/// including a `warp_cursor`), which a self-test uses to confirm the pointer
/// actually tracked a moved window. Needs no Accessibility permission.
#[cfg(target_os = "macos")]
pub fn cursor_global() -> Option<(f64, f64)> {
    use std::ffi::c_void;

    #[repr(C)]
    struct CGPoint {
        x: f64,
        y: f64,
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventCreate(source: *mut c_void) -> *mut c_void;
        fn CGEventGetLocation(event: *mut c_void) -> CGPoint;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(cf: *const c_void);
    }

    // SAFETY: a null-source `CGEventCreate` returns a +1 event carrying the current
    // cursor location; `CGEventGetLocation` reads it; `CFRelease` balances the +1.
    unsafe {
        let event = CGEventCreate(std::ptr::null_mut());
        if event.is_null() {
            return None;
        }
        let point = CGEventGetLocation(event);
        CFRelease(event.cast());
        Some((point.x, point.y))
    }
}

/// Non-macOS: no portable global-cursor read is wired up (the self-test is macOS).
#[cfg(not(target_os = "macos"))]
pub fn cursor_global() -> Option<(f64, f64)> {
    None
}

/// Stop the macOS scroll-**momentum** tail from reaching the overlays, so a
/// two-finger scroll-drag moves a window 1:1 with the fingers and then *stops on
/// lift* instead of coasting on past the gesture.
///
/// Why this exists: winit already hands us the system's accelerated
/// `scrollingDelta` (so [`crate::scroll_drag_delta`] moves the window on the
/// native acceleration curve, no custom math), but it *collapses* the trackpad's
/// touch phase and the OS momentum phase into one `MouseWheel` stream. Every
/// momentum-coast event arrives as a plain `TouchPhase::Moved`, indistinguishable
/// from a finger drag, so the overlay would keep flinging the window for the whole
/// coast. A window has no business coasting like scrolled content.
///
/// `-[NSEvent momentumPhase]` is the documented, canonical way to tell the coast
/// from a real scroll: it is non-`None` exactly for the momentum stream and
/// `None` during the user's physical scroll (and for a notched mouse wheel). See
/// Apple's "Handling Trackpad Events". winit does not expose it, so we install an
/// app-wide *local* `NSEvent` monitor (no Accessibility permission needed; it only
/// sees this app's own events) that returns `nil` for any scroll event with a
/// momentum phase, dropping it before winit's view ever queues a `MouseWheel`.
/// Physical-scroll and mouse-wheel events pass through untouched.
///
/// Call once after the event loop is running (e.g. from `resumed`), on the main
/// thread. The monitor is intentionally leaked: it lives for the whole process,
/// and there is no teardown point that would want it removed.
#[cfg(target_os = "macos")]
pub fn suppress_scroll_momentum() {
    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_app_kit::{NSEvent, NSEventMask};
    use std::ptr::NonNull;

    // Return the event to let it through, or null to swallow it. A scroll event
    // whose `momentumPhase` is set (raw bits != 0) is part of the inertial coast;
    // everything else (the physical two-finger scroll, a notched mouse wheel) is
    // passed through so winit handles it as usual.
    let handler = RcBlock::new(|event: NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit hands the monitor a live, autoreleased `NSEvent` for the
        // duration of the callback; we only read from it. `momentumPhase` is valid
        // for scroll-wheel events, which is all this monitor's mask selects.
        let ev = unsafe { event.as_ref() };
        if unsafe { ev.momentumPhase() }.0 != 0 {
            std::ptr::null_mut()
        } else {
            event.as_ptr()
        }
    });

    // SAFETY: a standard AppKit call on the main thread; the handler block is
    // `'static` (captures nothing) and matches the documented signature.
    let monitor: Option<Retained<AnyObject>> = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::ScrollWheel, &handler)
    };
    // Keep the monitor installed for the life of the process. Dropping the
    // returned token would tear the monitor down again.
    if let Some(monitor) = monitor {
        std::mem::forget(monitor);
    }
}

/// Non-macOS: X11/Wayland deliver no momentum-coast scroll stream to suppress
/// (kinetic scrolling, where present, is the compositor's, not extra events to a
/// client), so this is a no-op.
#[cfg(not(target_os = "macos"))]
pub fn suppress_scroll_momentum() {}

/// Attributes for a floating overlay window: transparent, borderless,
/// non-resizable, always on top, sized to `(w_px, h_px)` physical pixels and
/// placed at `pos` (logical points) when given.
pub fn float_attributes(
    title: &str,
    w_px: u32,
    h_px: u32,
    pos: Option<LogicalPosition<f64>>,
) -> WindowAttributes {
    let attrs = Window::default_attributes()
        .with_title(title)
        .with_transparent(true)
        .with_decorations(false)
        .with_resizable(false)
        .with_window_level(WindowLevel::AlwaysOnTop)
        .with_inner_size(PhysicalSize::new(w_px.max(1), h_px.max(1)));
    match pos {
        Some(p) => attrs.with_position(p),
        None => attrs,
    }
}

/// Pick an sRGB surface format, falling back to the first offered.
pub fn srgb_format(caps: &wgpu::SurfaceCapabilities) -> wgpu::TextureFormat {
    caps.formats
        .iter()
        .copied()
        .find(|f| f.is_srgb())
        .unwrap_or(caps.formats[0])
}

/// Pick a surface alpha mode that composites over the desktop. `Opaque` paints a
/// black box, so it is the last resort; Metal only offers `[Opaque,
/// PostMultiplied]`. Warns when only `Opaque` is available.
pub fn transparent_alpha_mode(caps: &wgpu::SurfaceCapabilities) -> wgpu::CompositeAlphaMode {
    let mode = [
        wgpu::CompositeAlphaMode::PostMultiplied,
        wgpu::CompositeAlphaMode::PreMultiplied,
        wgpu::CompositeAlphaMode::Inherit,
    ]
    .into_iter()
    .find(|m| caps.alpha_modes.contains(m))
    .unwrap_or(caps.alpha_modes[0]);
    if mode == wgpu::CompositeAlphaMode::Opaque {
        eprintln!(
            "overlay-core: no transparent surface alpha mode available ({:?}); \
             the overlay background will be opaque",
            caps.alpha_modes
        );
    }
    mode
}

/// A standard FIFO render-attachment surface configuration for an overlay window.
pub fn surface_config(
    format: wgpu::TextureFormat,
    alpha_mode: wgpu::CompositeAlphaMode,
    width: u32,
    height: u32,
) -> wgpu::SurfaceConfiguration {
    wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width: width.max(1),
        height: height.max(1),
        present_mode: wgpu::PresentMode::Fifo,
        alpha_mode,
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
    }
}

/// Request an adapter for `surface` and a default device/queue. Blocks; intended
/// for the one-time GPU bring-up on the first window.
pub fn request_adapter_device(
    instance: &wgpu::Instance,
    surface: &wgpu::Surface<'_>,
) -> (wgpu::Adapter, wgpu::Device, wgpu::Queue) {
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::default(),
        compatible_surface: Some(surface),
        force_fallback_adapter: false,
    }))
    .expect("request wgpu adapter");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("overlay.device"),
        ..Default::default()
    }))
    .expect("request wgpu device");
    (adapter, device, queue)
}

/// Build the overlay event loop with a user-event channel of type `T`. On macOS,
/// accessory activation keeps the overlay windows but drops the Dock icon and
/// app-switcher entry, so a background HUD takes no Dock slot.
#[cfg(target_os = "macos")]
pub fn build_event_loop<T>() -> Result<EventLoop<T>, winit::error::EventLoopError> {
    use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
    EventLoop::with_user_event()
        .with_activation_policy(ActivationPolicy::Accessory)
        .build()
}

#[cfg(not(target_os = "macos"))]
pub fn build_event_loop<T>() -> Result<EventLoop<T>, winit::error::EventLoopError> {
    EventLoop::with_user_event().build()
}

/// Raise `window` above its same-level siblings without taking keyboard focus, so
/// a hovered/active overlay paints over its neighbours instead of slipping under.
/// Windows sharing one `WindowLevel` stack by front-to-back order, so an
/// earlier-created window would otherwise sit beneath a later one.
///
/// `-[NSWindow orderFrontRegardless]` reorders without making the window key, so
/// the user's keyboard focus stays put: a passive HUD must never steal it, which
/// rules out winit's `focus_window`. winit exposes no non-activating raise, so we
/// reach the `NSWindow` through the raw AppKit handle.
#[cfg(target_os = "macos")]
pub fn raise_to_front(window: &Window) {
    use objc2_app_kit::NSView;
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return;
    };
    // SAFETY: `ns_view` points at the live NSView backing this winit window, kept
    // alive for the whole call by the caller's `Arc<Window>`, so the `&NSView`
    // borrow stays valid (dropped before we return). We run inside the winit
    // event loop on the main thread, as these MainThreadOnly types require.
    let view: &NSView = unsafe { appkit.ns_view.cast().as_ref() };
    if let Some(ns_window) = view.window() {
        unsafe { ns_window.orderFrontRegardless() };
    }
}

/// Non-macOS: X11 and Wayland give an app no non-activating raise among
/// same-level always-on-top windows, so stacking is left to the compositor.
#[cfg(not(target_os = "macos"))]
pub fn raise_to_front(_window: &Window) {}

/// Make `window` react to pointer hover even when another application is the
/// active one. An overlay runs under the Accessory activation policy, so it is
/// never the active app and never owns a key window. winit's built-in mouse
/// tracking uses a legacy `addTrackingRect:`, whose `mouseEntered:` /
/// `mouseMoved:` / `mouseExited:` only reach the active app's key window, so a
/// background overlay receives no `CursorEntered`/`CursorMoved`/`CursorLeft` and
/// its hover state (the whole-book grow, the page-turn arrow highlight, the boss
/// bar panel) never fires while the user works in another app: the normal case
/// for a desktop HUD. A button event still reaches a background window, which is
/// why a click worked while hover did not.
///
/// Adding an `NSTrackingArea` with `NSTrackingActiveAlways` to winit's content
/// view (which already implements those responder methods) routes hover to the
/// overlay regardless of which app is active. `NSTrackingInVisibleRect` makes the
/// area track the view's visible rect, so it follows resizes with no re-add.
#[cfg(target_os = "macos")]
pub fn enable_background_hover(window: &Window) {
    use objc2::runtime::AnyObject;
    use objc2::ClassType;
    use objc2_app_kit::{NSTrackingArea, NSTrackingAreaOptions, NSView};
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return;
    };
    // SAFETY: `ns_view` points at the live NSView backing this winit window, kept
    // alive for the whole call by the caller's `Arc<Window>`. We run on the main
    // thread inside the winit event loop, as these AppKit types require. The same
    // pointer yields both the typed `&NSView` (to add the area) and the
    // `&AnyObject` owner the area records (that view, which handles the events).
    // The two shared `&` borrows aliasing one Objective-C object is sound: objc2
    // objects are interior-mutable, so `&`-aliasing carries no exclusivity claim
    // (this mirrors `raise_to_front` above).
    let view: &NSView = unsafe { appkit.ns_view.cast().as_ref() };
    let owner: &AnyObject = unsafe { appkit.ns_view.cast().as_ref() };

    // A tracking area with `NSTrackingMouseMoved` delivers moves within it, but a
    // background window still needs to accept mouse-moved events for them to flow.
    if let Some(ns_window) = view.window() {
        ns_window.setAcceptsMouseMovedEvents(true);
    }

    let options = NSTrackingAreaOptions::NSTrackingMouseEnteredAndExited
        | NSTrackingAreaOptions::NSTrackingMouseMoved
        | NSTrackingAreaOptions::NSTrackingActiveAlways
        | NSTrackingAreaOptions::NSTrackingInVisibleRect;
    // The rect is ignored because of `NSTrackingInVisibleRect`.
    let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0));
    // SAFETY: a standard `NSTrackingArea` init; `view.addTrackingArea` retains the
    // area, so it outlives this `Retained` going out of scope.
    let area = unsafe {
        NSTrackingArea::initWithRect_options_owner_userInfo(
            NSTrackingArea::alloc(),
            rect,
            options,
            Some(owner),
            None,
        )
    };
    unsafe { view.addTrackingArea(&area) };
}

/// Non-macOS: X11/Wayland deliver pointer-motion events to a window without an
/// active-app gate, so winit's default tracking already drives hover.
#[cfg(not(target_os = "macos"))]
pub fn enable_background_hover(_window: &Window) {}

/// The main screen's usable area (excluding the menu bar and Dock) in winit's
/// top-left logical points as `(left, top, width, height)`, or `None` off macOS
/// or with no screen. Auto-placing an overlay within this rather than the full
/// display keeps it clear of the menu bar and Dock.
#[cfg(target_os = "macos")]
pub fn visible_frame_logical() -> Option<(f64, f64, f64, f64)> {
    use objc2_app_kit::NSScreen;
    use objc2_foundation::MainThreadMarker;

    // AppKit screen geometry is main-thread only; we are called from the winit
    // event loop, which is the main thread. Decline rather than risk UB if not.
    let mtm = MainThreadMarker::new()?;
    let screen = NSScreen::mainScreen(mtm)?;
    let full = screen.frame();
    let visible = screen.visibleFrame();
    // Cocoa frames use a bottom-left origin; convert the visible region's top edge
    // to a top-left inset (the menu bar height) for winit's coordinate space.
    let top = full.size.height - (visible.origin.y + visible.size.height);
    Some((visible.origin.x, top, visible.size.width, visible.size.height))
}

#[cfg(not(target_os = "macos"))]
pub fn visible_frame_logical() -> Option<(f64, f64, f64, f64)> {
    None
}
