//! The live overlay: one transparent, always-on-top, borderless window **per
//! bar**, each sized to just that bar. winit owns the windows and event loop;
//! the SQLite watcher runs on its own thread and wakes the loop with a fresh
//! `Vec<BossBar>` whenever the database changes.
//!
//! Because each window covers only its bar, the desktop is click-through
//! everywhere else automatically: there is simply no window to intercept the
//! mouse off a bar. Hovering a bar fires native `CursorEntered`/`CursorLeft`
//! (the bar paints opaque, the cursor becomes a grab hand); pressing it calls
//! the native `Window::drag_window`, so the OS owns the drag and the new
//! position is read back from `Moved` and persisted. No cursor polling, no
//! `set_cursor_hittest`, no coordinate reconciliation.

use std::collections::HashMap;
use std::f32::consts::TAU;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::DVec2;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{CursorIcon, Window, WindowId, WindowLevel};

use crate::bars::BossBar;
use crate::db;
use crate::render::{self, Renderer};

/// Top margin and vertical gap (logical points) for bars that have no pinned
/// position and fall back to the auto-stacked top-center column. `AUTO_TOP`
/// clears the macOS menu bar so the system does not nudge the window down (a
/// nudge would otherwise read as a move).
const AUTO_TOP: f64 = 40.0;
const AUTO_GAP: f64 = 6.0;

/// Hover grow/shrink time (seconds): an ease-out tween at the responsive end of
/// the feedback range. See the `animation` skill.
const GROW_SECS: f32 = 0.16;
/// Full breathing cycle (seconds) while hovered. ~2.6s reads calm and alive,
/// near a resting heart rate, rather than urgent.
const BREATHE_PERIOD: f32 = 2.6;
/// Animation frame budget while something is moving (~60 fps).
const FRAME: Duration = Duration::from_millis(16);

/// Ease-out cubic: fast start, soft landing. The default feedback curve.
fn ease_out_cubic(t: f32) -> f32 {
    let u = 1.0 - t.clamp(0.0, 1.0);
    1.0 - u * u * u
}

/// GPU state shared by every bar window: one device/queue and one renderer
/// (pipeline, sprite textures, font). Each window only owns its surface.
struct GpuCore {
    device: wgpu::Device,
    format: wgpu::TextureFormat,
    alpha_mode: wgpu::CompositeAlphaMode,
    renderer: Renderer,
}

/// One bar's window and its surface.
struct BarWin {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    bar: BossBar,
    /// Hover target: the pointer is currently over this bar.
    hovered: bool,
    /// Eased hover amount in `0..=1`, animated toward `hovered` each frame. Drives
    /// the grow + opacity; the breathing fades in with it.
    hover_anim: f32,
    /// Timestamp of the last animation step, for frame-rate-independent easing.
    last: Instant,
    /// True between a left-press and its release: only then does a `Moved`
    /// count as a user drag worth persisting. Window placement nudges by the OS
    /// (e.g. clamping below the menu bar) arrive with this false and are ignored.
    dragging: bool,
    /// Last position we know the window holds (logical points): what we set, or
    /// where the OS placed it. Lets `Moved` skip echoes of our own placement.
    self_set: Option<LogicalPosition<f64>>,
}

impl BarWin {
    /// Still moving: easing toward/away from hover, or breathing while hovered.
    fn animating(&self) -> bool {
        self.hovered || self.hover_anim > 0.0
    }
}

pub struct App {
    db: PathBuf,
    /// Logical pixel scale; multiplied by the monitor scale factor so the bars
    /// look the same physical size on HiDPI and standard displays.
    base_scale: u32,
    proxy: EventLoopProxy<Vec<BossBar>>,
    instance: wgpu::Instance,
    core: Option<GpuCore>,
    wins: HashMap<i64, BarWin>,

    /// Monitor logical size, for centering auto-stacked bars.
    mon_logical: (f64, f64),
    scale_factor: f64,
    /// Physical sprite scale, `round(base_scale * scale_factor)`.
    scale: u32,
    /// App start, the zero point for the continuous breathing phase.
    start: Instant,
    ready: bool,
}

impl App {
    /// Build a surface for `window`, creating the shared GPU core on first use.
    fn make_surface(
        &mut self,
        window: &Arc<Window>,
    ) -> (wgpu::Surface<'static>, wgpu::SurfaceConfiguration) {
        let surface = self
            .instance
            .create_surface(window.clone())
            .expect("create surface");

        if self.core.is_none() {
            let adapter = pollster::block_on(self.instance.request_adapter(
                &wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::default(),
                    compatible_surface: Some(&surface),
                    force_fallback_adapter: false,
                },
            ))
            .expect("request wgpu adapter");
            let (device, queue) = pollster::block_on(adapter.request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("bossbar.device"),
                    ..Default::default()
                },
            ))
            .expect("request device");

            let caps = surface.get_capabilities(&adapter);
            let format = caps
                .formats
                .iter()
                .copied()
                .find(|f| f.is_srgb())
                .unwrap_or(caps.formats[0]);
            // Must composite over the desktop, so never `Opaque` (which paints a
            // black box). Metal only offers `[Opaque, PostMultiplied]`.
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
                     ({:?}); bars will have an opaque background",
                    caps.alpha_modes
                );
            }

            let renderer = Renderer::new(device.clone(), queue, format, self.scale.max(1));
            self.core = Some(GpuCore {
                device,
                format,
                alpha_mode,
                renderer,
            });
        }

        let core = self.core.as_ref().expect("core just initialized");
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: core.format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: core.alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&core.device, &config);
        (surface, config)
    }

    /// Auto-stacked default position (logical points) for the `slot`-th bar that
    /// has no pinned location.
    fn auto_pos(&self, slot: usize, w_px: u32, h_px: u32) -> LogicalPosition<f64> {
        let wl = w_px as f64 / self.scale_factor;
        let hl = h_px as f64 / self.scale_factor;
        let x = ((self.mon_logical.0 - wl) / 2.0).max(0.0);
        let y = AUTO_TOP + slot as f64 * (hl + AUTO_GAP);
        LogicalPosition::new(x, y)
    }

    fn create_win(
        &mut self,
        event_loop: &ActiveEventLoop,
        bar: BossBar,
        slot: usize,
    ) -> Option<BarWin> {
        let has_title = !bar.title.is_empty();
        let (w_px, h_px) = render::bar_window_px(self.scale, has_title);
        let pos = match bar.pos {
            Some(p) => LogicalPosition::new(p.x, p.y),
            None => self.auto_pos(slot, w_px, h_px),
        };
        let attrs = Window::default_attributes()
            .with_title("Boss Bar")
            .with_transparent(true)
            .with_decorations(false)
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_inner_size(PhysicalSize::new(w_px, h_px))
            .with_position(pos);
        let window = Arc::new(event_loop.create_window(attrs).ok()?);
        let (surface, config) = self.make_surface(&window);
        window.request_redraw();
        Some(BarWin {
            window,
            surface,
            config,
            bar,
            hovered: false,
            hover_anim: 0.0,
            last: Instant::now(),
            dragging: false,
            self_set: Some(pos),
        })
    }

    /// Diff the DB's visible bars against the live windows: drop windows for
    /// gone bars, create them for new bars, update data and (externally changed)
    /// position for existing ones.
    fn reconcile(&mut self, event_loop: &ActiveEventLoop, bars: Vec<BossBar>) {
        self.wins.retain(|id, _| bars.iter().any(|b| b.id == *id));

        let mut slot = 0usize;
        for b in &bars {
            let this_slot = if b.pos.is_none() {
                let s = slot;
                slot += 1;
                s
            } else {
                0
            };

            if let Some(win) = self.wins.get_mut(&b.id) {
                let title_presence_changed = win.bar.title.is_empty() != b.title.is_empty();
                win.bar = b.clone();
                // Honor a position set in the DB by something other than our own
                // drag (e.g. the CLI), but ignore the echo of our last move.
                if let Some(p) = b.pos {
                    let lp = LogicalPosition::new(p.x, p.y);
                    if win.self_set != Some(lp) {
                        win.window.set_outer_position(lp);
                        win.self_set = Some(lp);
                    }
                }
                if title_presence_changed {
                    let (w_px, h_px) = render::bar_window_px(self.scale, !b.title.is_empty());
                    let _ = win.window.request_inner_size(PhysicalSize::new(w_px, h_px));
                }
                win.window.request_redraw();
            } else if let Some(win) = self.create_win(event_loop, b.clone(), this_slot) {
                self.wins.insert(b.id, win);
            }
        }
    }

    fn bar_id_for(&self, wid: WindowId) -> Option<i64> {
        self.wins
            .iter()
            .find(|(_, w)| w.window.id() == wid)
            .map(|(id, _)| *id)
    }

    fn render_id(&mut self, id: i64) {
        let now = Instant::now();
        // Continuous breathing phase: a sine over the whole run, never reset, so
        // it never snaps when a bar starts or stops being hovered.
        let breathe = (TAU * now.duration_since(self.start).as_secs_f32() / BREATHE_PERIOD).sin();

        // Advance this bar's eased hover amount toward its target, then map it
        // through ease-out for the visible grow/opacity.
        let hover = {
            let Some(win) = self.wins.get_mut(&id) else {
                return;
            };
            let dt = now.duration_since(win.last).as_secs_f32().min(0.05);
            win.last = now;
            let target = if win.hovered { 1.0 } else { 0.0 };
            let step = dt / GROW_SECS;
            win.hover_anim = if win.hover_anim < target {
                (win.hover_anim + step).min(target)
            } else {
                (win.hover_anim - step).max(target)
            };
            ease_out_cubic(win.hover_anim)
        };

        let Some(core) = self.core.as_mut() else {
            return;
        };
        let Some(win) = self.wins.get_mut(&id) else {
            return;
        };
        let frame = match win.surface.get_current_texture() {
            Ok(f) => f,
            Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                win.surface.configure(&core.device, &win.config);
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
        let _ = core.renderer.render_one(
            &view,
            win.config.width,
            win.config.height,
            &win.bar,
            hover,
            breathe,
        );
        frame.present();
    }

    /// A window moved. Persist it only when the move came from a user drag;
    /// otherwise (initial placement, OS menu-bar clamp) just record where the
    /// window actually sits so a later drag is measured from the right spot.
    fn on_moved(&mut self, id: i64, pos: PhysicalPosition<i32>) {
        let Some(win) = self.wins.get_mut(&id) else {
            return;
        };
        let logical: LogicalPosition<f64> = pos.to_logical(win.window.scale_factor());
        let echo = win
            .self_set
            .is_some_and(|ss| (ss.x - logical.x).abs() < 0.5 && (ss.y - logical.y).abs() < 0.5);
        if echo {
            return;
        }
        win.self_set = Some(logical);
        if !win.dragging {
            return; // OS-initiated placement, not a user drag
        }
        let dv = DVec2::new(logical.x, logical.y);
        win.bar.pos = Some(dv);
        if let Err(e) = db::set_position(&self.db, id, dv) {
            eprintln!("bossbar-overlay: save position failed: {e}");
        }
    }
}

impl ApplicationHandler<Vec<BossBar>> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.ready {
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
        self.scale = ((self.base_scale as f64) * scale_factor).round().max(1.0) as u32;
        self.mon_logical = (mon_w as f64 / scale_factor, mon_h as f64 / scale_factor);
        self.ready = true;

        let bars = db::read_once(&self.db).unwrap_or_default();
        self.reconcile(event_loop, bars);

        // Wake the loop with fresh bars on every DB change; a send error means
        // the loop is gone, which stops the watcher thread.
        let proxy = self.proxy.clone();
        db::spawn_watcher(self.db.clone(), move |bars| proxy.send_event(bars).is_ok());
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, bars: Vec<BossBar>) {
        self.reconcile(event_loop, bars);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, wid: WindowId, event: WindowEvent) {
        let Some(id) = self.bar_id_for(wid) else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => {
                self.wins.remove(&id);
                if self.wins.is_empty() {
                    event_loop.exit();
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(win) = self.wins.get_mut(&id) {
                    win.config.width = size.width.max(1);
                    win.config.height = size.height.max(1);
                    if let Some(core) = self.core.as_ref() {
                        win.surface.configure(&core.device, &win.config);
                    }
                }
                self.render_id(id);
            }
            WindowEvent::RedrawRequested => self.render_id(id),
            WindowEvent::CursorEntered { .. } => {
                if let Some(win) = self.wins.get_mut(&id) {
                    win.hovered = true;
                    win.window.set_cursor(CursorIcon::Grab);
                }
                self.render_id(id);
            }
            WindowEvent::CursorLeft { .. } => {
                if let Some(win) = self.wins.get_mut(&id) {
                    win.hovered = false;
                    win.window.set_cursor(CursorIcon::Default);
                }
                self.render_id(id);
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if let Some(win) = self.wins.get_mut(&id) {
                    win.dragging = true;
                    win.window.set_cursor(CursorIcon::Grabbing);
                    if let Err(e) = win.window.drag_window() {
                        eprintln!("bossbar-overlay: drag failed: {e}");
                    }
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => {
                if let Some(win) = self.wins.get_mut(&id) {
                    win.dragging = false;
                    win.window.set_cursor(if win.hovered {
                        CursorIcon::Grab
                    } else {
                        CursorIcon::Default
                    });
                }
            }
            WindowEvent::Moved(pos) => self.on_moved(id, pos),
            _ => {}
        }
    }

    /// Keep redrawing while any bar is animating (growing, shrinking, or
    /// breathing), capped to ~60 fps; otherwise let the loop sleep until the
    /// next event so an idle overlay costs nothing.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let mut animating = false;
        for win in self.wins.values() {
            if win.animating() {
                animating = true;
                win.window.request_redraw();
            }
        }
        event_loop.set_control_flow(if animating {
            ControlFlow::WaitUntil(Instant::now() + FRAME)
        } else {
            ControlFlow::Wait
        });
    }
}

/// Run the overlay event loop. Blocks until the last window closes.
pub fn run(db: PathBuf, base_scale: u32) -> Result<(), Box<dyn std::error::Error>> {
    let event_loop = build_event_loop()?;
    let proxy = event_loop.create_proxy();
    let mut app = App {
        db,
        base_scale: base_scale.max(1),
        proxy,
        instance: wgpu::Instance::default(),
        core: None,
        wins: HashMap::new(),
        mon_logical: (1920.0, 1080.0),
        scale_factor: 1.0,
        scale: base_scale.max(1),
        start: Instant::now(),
        ready: false,
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn build_event_loop() -> Result<EventLoop<Vec<BossBar>>, winit::error::EventLoopError> {
    use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
    // Accessory keeps the overlay windows but drops the Dock icon and
    // app-switcher entry, so a background HUD does not take a Dock slot.
    EventLoop::with_user_event()
        .with_activation_policy(ActivationPolicy::Accessory)
        .build()
}

#[cfg(not(target_os = "macos"))]
fn build_event_loop() -> Result<EventLoop<Vec<BossBar>>, winit::error::EventLoopError> {
    EventLoop::with_user_event().build()
}
