//! Float-window mechanics shared by every overlay: a transparent, borderless,
//! always-on-top window with no Dock presence, plus the surface and adapter
//! plumbing and a non-activating raise. The desktop stays click-through wherever
//! no overlay window sits, because there is simply no window there to intercept
//! the pointer.

use winit::dpi::{LogicalPosition, PhysicalSize};
use winit::event_loop::EventLoop;
use winit::window::{Window, WindowAttributes, WindowLevel};

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
