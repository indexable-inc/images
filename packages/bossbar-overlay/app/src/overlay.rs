//! The live overlay: a transparent, always-on-top window over the screen.
//! winit owns the window and event loop; the SQLite watcher runs on its own
//! thread and wakes the loop with a fresh `Vec<BossBar>` whenever the database
//! changes.
//!
//! The window is click-through by default so the desktop underneath stays
//! usable. On macOS a background thread polls the global cursor (CoreGraphics),
//! and the moment it crosses a bar the window flips to capture the pointer:
//! that bar highlights, and a drag repositions it (persisted to the DB). Off
//! the bars the window returns to click-through, so only the bars themselves
//! intercept the mouse. Other platforms keep the static click-through overlay.

use std::path::PathBuf;
use std::sync::Arc;

use glam::{DVec2, Vec2, vec2};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::window::{CursorIcon, Window, WindowLevel};

use crate::bars::BossBar;
use crate::db;
use crate::render::Renderer;

/// Fraction of the monitor height the overlay covers on platforms without the
/// interactive (drag-anywhere) path; the window is click-through and this just
/// bounds where bars can paint.
#[cfg(not(target_os = "macos"))]
const HEIGHT_FRACTION: f64 = 0.45;

/// Loop wake-ups: a fresh set of bars from the DB watcher, or the latest global
/// cursor position from the macOS poll thread (physical pixels).
enum Ev {
    Bars(Vec<BossBar>),
    Cursor(Vec2),
}

/// An in-progress drag: which bar, and the pointer's offset from the bar's
/// origin at grab time, so the bar tracks the cursor without jumping.
#[derive(Clone, Copy)]
struct Drag {
    id: i64,
    grab: Vec2,
}

struct Gpu {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
}

pub struct App {
    db: PathBuf,
    /// Logical pixel scale; multiplied by the monitor scale factor so the bars
    /// look the same physical size on HiDPI and standard displays.
    base_scale: u32,
    proxy: EventLoopProxy<Ev>,
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    bars: Vec<BossBar>,

    /// Display backing-scale factor, captured on resume (e.g. 2.0 on Retina).
    scale_factor: f64,
    /// Whether the window currently captures the pointer (`set_cursor_hittest`).
    /// Default is click-through; it flips on only while the cursor is on a bar.
    interactive: bool,
    /// Bar under the pointer, drawn opaque for feedback.
    hovered: Option<i64>,
    /// Active drag, if the pointer is held down on a bar.
    dragging: Option<Drag>,
    /// Last known pointer position, in physical pixels.
    cursor: Vec2,
}

impl App {
    fn renderer(&self) -> Option<&Renderer> {
        self.gpu.as_ref().map(|g| &g.renderer)
    }

    fn width(&self) -> f32 {
        self.gpu.as_ref().map(|g| g.config.width as f32).unwrap_or(0.0)
    }

    fn bar_index(&self, id: i64) -> Option<usize> {
        self.bars.iter().position(|b| b.id == id)
    }

    /// The bar under a physical-pixel point, via the renderer's shared layout.
    fn hit_test(&self, point: Vec2) -> Option<i64> {
        self.renderer()?.hit(&self.bars, self.width(), point)
    }

    /// Toggle pointer capture. Off (the default) lets clicks fall through to the
    /// desktop; on lets the window receive hover, click, and drag on a bar.
    fn set_interactive(&mut self, on: bool) {
        if self.interactive == on {
            return;
        }
        if let Some(w) = &self.window {
            // Wayland does not support this; the error is ignored there, leaving
            // the overlay non-interactive rather than crashing.
            let _ = w.set_cursor_hittest(on);
        }
        self.interactive = on;
    }

    fn set_icon(&self, icon: CursorIcon) {
        if let Some(w) = &self.window {
            w.set_cursor(icon);
        }
    }

    fn request_redraw(&self) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn render(&mut self) {
        // The hovered (or actively dragged) bar paints opaque.
        let highlight = self.dragging.map(|d| d.id).or(self.hovered);
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        let frame = match gpu.surface.get_current_texture() {
            Ok(f) => f,
            Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                gpu.surface.configure(&gpu.device, &gpu.config);
                return;
            }
            Err(e) => {
                eprintln!("bossbar-overlay: surface error: {e:?}");
                return;
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let _ = gpu
            .renderer
            .render(&view, gpu.config.width, gpu.config.height, &self.bars, highlight);
        frame.present();
    }
}

impl ApplicationHandler<Ev> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let monitor = event_loop
            .primary_monitor()
            .or_else(|| event_loop.available_monitors().next());
        let (mon_w, mon_h, scale_factor) = match &monitor {
            Some(m) => (m.size().width, m.size().height, m.scale_factor()),
            None => (1920, 1080, 1.0),
        };
        self.scale_factor = scale_factor;

        // macOS drives the interactive path, so the window spans the whole
        // screen and a bar can be dragged anywhere. Elsewhere it stays the
        // click-through top strip.
        #[cfg(target_os = "macos")]
        let window_size = PhysicalSize::new(mon_w.max(1), mon_h.max(1));
        #[cfg(not(target_os = "macos"))]
        let window_size = PhysicalSize::new(
            mon_w.max(1),
            (((mon_h as f64) * HEIGHT_FRACTION) as u32).max(1),
        );

        let attrs = Window::default_attributes()
            .with_title("Boss Bar Overlay")
            .with_transparent(true)
            .with_decorations(false)
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_inner_size(window_size)
            .with_position(PhysicalPosition::new(0, 0));
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("create overlay window"),
        );
        // Start click-through: every click falls to whatever is underneath until
        // the cursor crosses a bar. Not all backends support this (notably
        // Wayland); ignore the error there.
        let _ = window.set_cursor_hittest(false);

        let instance = wgpu::Instance::default();
        let surface = instance
            .create_surface(window.clone())
            .expect("create surface");
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            }))
            .expect("request wgpu adapter");
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("bossbar.device"),
                ..Default::default()
            }))
            .expect("request device");

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);
        // The framebuffer must composite over the desktop, so we must NOT pick
        // `Opaque` (which makes the surface layer opaque and paints a black box
        // behind the bars). Metal only offers `[Opaque, PostMultiplied]`, so
        // prefer any non-opaque mode in preference order.
        let alpha_mode = [
            wgpu::CompositeAlphaMode::PostMultiplied,
            wgpu::CompositeAlphaMode::PreMultiplied,
            wgpu::CompositeAlphaMode::Inherit,
        ]
        .into_iter()
        .find(|m| caps.alpha_modes.contains(m))
        .unwrap_or(caps.alpha_modes[0]);
        if alpha_mode == wgpu::CompositeAlphaMode::Opaque {
            eprintln!(
                "bossbar-overlay: no transparent surface alpha mode available \
                 ({:?}); the overlay background will be opaque",
                caps.alpha_modes
            );
        }
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let scale = ((self.base_scale as f64) * scale_factor).round() as u32;
        let renderer =
            Renderer::new(device.clone(), queue, format, scale.max(1), scale_factor as f32);

        self.bars = db::read_once(&self.db).unwrap_or_default();
        self.gpu = Some(Gpu {
            surface,
            device,
            config,
            renderer,
        });
        self.window = Some(window.clone());

        // Wake the loop with fresh bars on every DB change; a send error means
        // the loop is gone, which stops the watcher thread.
        let proxy = self.proxy.clone();
        db::spawn_watcher(self.db.clone(), move |bars| {
            proxy.send_event(Ev::Bars(bars)).is_ok()
        });

        // Track the global cursor so the click-through window can wake up the
        // instant the pointer reaches a bar (macOS only).
        spawn_cursor_poll(self.proxy.clone(), scale_factor);

        window.request_redraw();
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: Ev) {
        match event {
            Ev::Bars(bars) => {
                self.bars = bars;
                self.request_redraw();
            }
            Ev::Cursor(p) => {
                self.cursor = p;
                // While the window already captures the pointer, winit's own
                // CursorMoved drives hover and drag; the poll only matters for
                // waking the click-through window when a bar is first entered.
                if self.interactive || self.dragging.is_some() {
                    return;
                }
                if let Some(id) = self.hit_test(p) {
                    self.hovered = Some(id);
                    self.set_interactive(true);
                    self.set_icon(CursorIcon::Grab);
                    self.request_redraw();
                }
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(gpu) = self.gpu.as_mut() {
                    gpu.config.width = size.width.max(1);
                    gpu.config.height = size.height.max(1);
                    gpu.surface.configure(&gpu.device, &gpu.config);
                }
                self.request_redraw();
            }
            WindowEvent::RedrawRequested => self.render(),
            WindowEvent::CursorMoved { position, .. } => {
                let p = vec2(position.x as f32, position.y as f32);
                self.cursor = p;
                if let Some(d) = self.dragging {
                    // Move the dragged bar to follow the cursor, storing its new
                    // anchor in logical points so it survives scale changes.
                    let origin = p - d.grab;
                    let sf = self.scale_factor as f32;
                    if let Some(i) = self.bar_index(d.id) {
                        self.bars[i].pos =
                            Some(DVec2::new((origin.x / sf) as f64, (origin.y / sf) as f64));
                        self.request_redraw();
                    }
                } else {
                    match self.hit_test(p) {
                        Some(id) => {
                            if self.hovered != Some(id) {
                                self.hovered = Some(id);
                                self.request_redraw();
                            }
                            self.set_icon(CursorIcon::Grab);
                        }
                        None => {
                            // Left every bar: hand the pointer back to the desktop.
                            self.hovered = None;
                            self.set_interactive(false);
                            self.set_icon(CursorIcon::Default);
                            self.request_redraw();
                        }
                    }
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => match state {
                ElementState::Pressed => {
                    if let Some(id) = self.hovered {
                        let origin = self
                            .renderer()
                            .and_then(|r| r.origin_of(&self.bars, self.width(), id));
                        if let Some(origin) = origin {
                            self.dragging = Some(Drag {
                                id,
                                grab: self.cursor - origin,
                            });
                            self.set_icon(CursorIcon::Grabbing);
                        }
                    }
                }
                ElementState::Released => {
                    if let Some(d) = self.dragging.take() {
                        if let Some(i) = self.bar_index(d.id) {
                            if let Some(pos) = self.bars[i].pos {
                                if let Err(e) = db::set_position(&self.db, d.id, pos) {
                                    eprintln!("bossbar-overlay: save position failed: {e}");
                                }
                            }
                        }
                        // Re-resolve hover at the drop point: stay live if still
                        // on a bar, otherwise return to click-through.
                        match self.hit_test(self.cursor) {
                            Some(id) => {
                                self.hovered = Some(id);
                                self.set_icon(CursorIcon::Grab);
                            }
                            None => {
                                self.hovered = None;
                                self.set_interactive(false);
                                self.set_icon(CursorIcon::Default);
                            }
                        }
                        self.request_redraw();
                    }
                }
            },
            WindowEvent::CursorLeft { .. } => {
                if self.dragging.is_none() {
                    self.hovered = None;
                    self.set_interactive(false);
                    self.set_icon(CursorIcon::Default);
                    self.request_redraw();
                }
            }
            _ => {}
        }
    }
}

/// Run the overlay event loop. Blocks until the window closes.
pub fn run(db: PathBuf, base_scale: u32) -> Result<(), Box<dyn std::error::Error>> {
    let event_loop = build_event_loop()?;
    let proxy = event_loop.create_proxy();
    let mut app = App {
        db,
        base_scale: base_scale.max(1),
        proxy,
        window: None,
        gpu: None,
        bars: Vec::new(),
        scale_factor: 1.0,
        interactive: false,
        hovered: None,
        dragging: None,
        cursor: Vec2::ZERO,
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}

/// Poll the global cursor location and forward it to the event loop so the
/// click-through window can detect when the pointer reaches a bar. macOS only;
/// reads the HID pointer position with no accessibility permission.
#[cfg(target_os = "macos")]
fn spawn_cursor_poll(proxy: EventLoopProxy<Ev>, scale_factor: f64) {
    use core_graphics::event::CGEvent;
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    use std::time::Duration;

    std::thread::spawn(move || {
        let Ok(source) = CGEventSource::new(CGEventSourceStateID::HIDSystemState) else {
            eprintln!("bossbar-overlay: cursor source unavailable; overlay stays click-through");
            return;
        };
        loop {
            // CGEvent locations are top-left logical points; scale to the
            // framebuffer's physical pixels to match the window/render space.
            if let Ok(ev) = CGEvent::new(source.clone()) {
                let p = ev.location();
                let phys = vec2((p.x * scale_factor) as f32, (p.y * scale_factor) as f32);
                if proxy.send_event(Ev::Cursor(phys)).is_err() {
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(16));
        }
    });
}

/// Non-macOS: no global cursor source wired up, so the overlay stays
/// click-through and non-interactive.
#[cfg(not(target_os = "macos"))]
fn spawn_cursor_poll(_proxy: EventLoopProxy<Ev>, _scale_factor: f64) {}

#[cfg(target_os = "macos")]
fn build_event_loop() -> Result<EventLoop<Ev>, winit::error::EventLoopError> {
    use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
    // Accessory keeps the overlay window but drops the Dock icon and
    // app-switcher entry, so a background HUD does not take a Dock slot.
    EventLoop::with_user_event()
        .with_activation_policy(ActivationPolicy::Accessory)
        .build()
}

#[cfg(not(target_os = "macos"))]
fn build_event_loop() -> Result<EventLoop<Ev>, winit::error::EventLoopError> {
    EventLoop::with_user_event().build()
}
