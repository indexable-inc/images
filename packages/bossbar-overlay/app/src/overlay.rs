//! The live overlay: a transparent, always-on-top, click-through window
//! spanning the top of the primary monitor. winit owns the window and event
//! loop; the SQLite watcher runs on its own thread and wakes the loop with a
//! fresh `Vec<BossBar>` user event whenever the database changes.

use std::path::PathBuf;
use std::sync::Arc;

use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowLevel};

use crate::bars::BossBar;
use crate::db;
use crate::render::Renderer;

/// Fraction of the monitor height the overlay window covers from the top. The
/// window itself is click-through; this just bounds where bars can paint.
const HEIGHT_FRACTION: f64 = 0.45;

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
    proxy: EventLoopProxy<Vec<BossBar>>,
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    bars: Vec<BossBar>,
}

impl App {
    fn render(&mut self) {
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
            .render(&view, gpu.config.width, gpu.config.height, &self.bars);
        frame.present();
    }
}

impl ApplicationHandler<Vec<BossBar>> for App {
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
        let height = ((mon_h as f64) * HEIGHT_FRACTION) as u32;

        let attrs = Window::default_attributes()
            .with_title("Boss Bar Overlay")
            .with_transparent(true)
            .with_decorations(false)
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_inner_size(PhysicalSize::new(mon_w.max(1), height.max(1)))
            .with_position(PhysicalPosition::new(0, 0));
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("create overlay window"),
        );
        // Let every click fall through to whatever is underneath. Not all
        // backends support this (notably Wayland); ignore the error there.
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
        let renderer = Renderer::new(device.clone(), queue, format, scale.max(1));

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
        db::spawn_watcher(self.db.clone(), move |bars| proxy.send_event(bars).is_ok());

        window.request_redraw();
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, bars: Vec<BossBar>) {
        self.bars = bars;
        if let Some(w) = &self.window {
            w.request_redraw();
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
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => self.render(),
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
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn build_event_loop() -> Result<EventLoop<Vec<BossBar>>, winit::error::EventLoopError> {
    use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
    // Accessory keeps the overlay window but drops the Dock icon and
    // app-switcher entry, so a background HUD does not take a Dock slot.
    EventLoop::with_user_event()
        .with_activation_policy(ActivationPolicy::Accessory)
        .build()
}

#[cfg(not(target_os = "macos"))]
fn build_event_loop() -> Result<EventLoop<Vec<BossBar>>, winit::error::EventLoopError> {
    EventLoop::with_user_event().build()
}
