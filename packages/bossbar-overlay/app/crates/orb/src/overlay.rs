//! The live experience-orb overlay: one transparent, always-on-top, borderless
//! window holding a single Minecraft orb that gently bobs and shimmers. winit owns
//! the window and event loop; the SQLite watcher runs on its own thread and wakes
//! the loop with a fresh [`Orb`] on any DB change.
//!
//! Off the window the desktop is click-through (there is no window there). On it:
//! hovering grows the orb and raises it above other overlays; dragging moves it
//! (the OS owns the drag via `Window::drag_window`, the position is read back and
//! persisted); a two-finger trackpad scroll also moves it
//! ([`overlay_core::scroll_drag_delta`]); a click that was not a drag opens the
//! orb's URL if it has one; right-click closes it. The orb bobs and shimmers
//! continuously, so the loop animates at a gentle ~30 fps rather than sleeping.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use overlay_core::glam::DVec2;
use overlay_core::wgpu;
use overlay_core::winit::application::ApplicationHandler;
use overlay_core::winit::dpi::{LogicalPosition, PhysicalPosition};
use overlay_core::winit::event::{
    ElementState, MouseButton, MouseScrollDelta, TouchPhase, WindowEvent,
};
use overlay_core::winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use overlay_core::winit::window::{CursorIcon, Window, WindowId};
use overlay_core::{anim, window as ocwin, DragClick, Gpu, HoverAnim};

use crate::db;
use crate::orb::Orb;
use crate::scene::{self, OrbTexture};

/// Pointer travel (physical px) past which a press becomes a window drag.
const DRAG_THRESHOLD: f64 = 5.0;
/// How long the window must sit still after its last move before an externally
/// read position is applied again, so the watcher's lagged read-back of our own
/// drag never snaps it back.
const SETTLE: Duration = Duration::from_millis(700);
/// Hover grow/shrink time: an ease-out tween at the responsive end, matching the
/// other overlays.
const GROW: Duration = Duration::from_millis(160);
/// Largest animation step a single frame may apply, so a stall does not jump the
/// motion; frames are otherwise ~33ms.
const MAX_STEP: Duration = Duration::from_millis(50);
/// Frame budget for the continuous bob + shimmer (~30 fps). The orb is alive by
/// nature, so it animates always; a gentle 30 fps keeps the cost of one small
/// quad negligible.
const FRAME: Duration = Duration::from_millis(33);
/// One full shimmer (colour pulse) cycle.
const SHIMMER_PERIOD: Duration = Duration::from_millis(2000);
/// One full bob (vertical float) cycle; offset from the shimmer so the two never
/// lock into one motion.
const BOB_PERIOD: Duration = Duration::from_millis(2600);

/// Open the orb's click URL with the platform opener, detached so the overlay
/// never blocks on the launch. A spawn failure is reported but not fatal.
fn open_url(url: &str) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    if let Err(e) = std::process::Command::new(opener).arg(url).spawn() {
        eprintln!("xp-orb-overlay: failed to open {url}: {e}");
    }
}

struct WinState {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    texture: OrbTexture,
    /// Press/drag/click disambiguation for the left-button gesture.
    gesture: DragClick,
    /// The pointer is on the orb: drives the grow.
    hovered: bool,
    /// Hover amount advanced toward `hovered` each frame.
    hover_anim: HoverAnim,
    /// Timestamp of the last animation step, for frame-rate-independent easing.
    last: Instant,
    /// Last position we know the window holds (logical points): what we set, or
    /// where the OS placed it. Lets `Moved` skip echoes of our own placement.
    self_set: Option<LogicalPosition<f64>>,
    /// When the window last moved during a drag/scroll, for the settle guard.
    last_move: Instant,
}

pub struct App {
    db: PathBuf,
    base_scale: u32,
    proxy: EventLoopProxy<Orb>,
    instance: wgpu::Instance,
    gpu: Option<Gpu>,
    win: Option<WinState>,
    orb: Orb,
    mon_logical: (f64, f64),
    scale_factor: f64,
    /// Physical sprite scale, `round(base_scale * scale_factor)`.
    scale: u32,
    /// App start, the zero point for the continuous shimmer/bob phases.
    start: Instant,
    ready: bool,
}

impl App {
    /// Auto-centered window position (logical points) within the screen's usable
    /// area so the orb clears the menu bar and Dock.
    fn center_pos(&self) -> LogicalPosition<f64> {
        let (w_px, h_px) = scene::orb_window_px(self.scale);
        let wl = w_px as f64 / self.scale_factor;
        let hl = h_px as f64 / self.scale_factor;
        let (left, top, vw, vh) = ocwin::visible_frame_logical()
            .unwrap_or((0.0, 0.0, self.mon_logical.0, self.mon_logical.1));
        LogicalPosition::new(
            left + ((vw - wl) * 0.5).max(0.0),
            top + ((vh - hl) * 0.5).max(0.0),
        )
    }

    fn create_window(&mut self, event_loop: &ActiveEventLoop) {
        let (w_px, h_px) = scene::orb_window_px(self.scale);
        let pos = self
            .orb
            .pos
            .map(|p| LogicalPosition::new(p.x, p.y))
            .unwrap_or_else(|| self.center_pos());
        let attrs = ocwin::float_attributes("XP Orb", w_px, h_px, Some(pos));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("xp-orb-overlay: create window failed: {e}");
                event_loop.exit();
                return;
            }
        };
        // The orb is a background (accessory) window, so hover only reaches it with
        // an always-active tracking area.
        ocwin::enable_background_hover(&window);

        let surface = self
            .instance
            .create_surface(window.clone())
            .expect("create surface");
        let (adapter, device, queue) = ocwin::request_adapter_device(&self.instance, &surface);
        let caps = surface.get_capabilities(&adapter);
        let format = ocwin::srgb_format(&caps);
        let alpha = ocwin::transparent_alpha_mode(&caps);

        let mut gpu = Gpu::new(device, queue, format);
        let texture = scene::register(&mut gpu);

        let size = window.inner_size();
        let config = ocwin::surface_config(format, alpha, size.width, size.height);
        surface.configure(gpu.device(), &config);
        window.request_redraw();

        self.gpu = Some(gpu);
        self.win = Some(WinState {
            window,
            surface,
            config,
            texture,
            gesture: DragClick::new(DRAG_THRESHOLD),
            hovered: false,
            hover_anim: HoverAnim::default(),
            last: Instant::now(),
            self_set: Some(pos),
            last_move: Instant::now() - SETTLE,
        });
    }

    fn render(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.start);
        // Continuous phases over the whole run so they never snap: shimmer in
        // 0..=1, bob in -1..=1.
        let shimmer = (anim::breathe(elapsed, SHIMMER_PERIOD) + 1.0) * 0.5;
        let bob = anim::breathe(elapsed, BOB_PERIOD);

        let Some(win) = self.win.as_mut() else {
            return;
        };
        let (cw, ch) = (win.config.width, win.config.height);
        let dt = now.duration_since(win.last).min(MAX_STEP);
        win.last = now;
        win.hover_anim
            .approach(if win.hovered { 1.0 } else { 0.0 }, dt, GROW);
        let hover = win.hover_anim.eased();

        let Some(gpu) = self.gpu.as_ref() else {
            return;
        };
        let frame = match win.surface.get_current_texture() {
            Ok(f) => f,
            Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                win.surface.configure(gpu.device(), &win.config);
                return;
            }
            Err(e) => {
                eprintln!("xp-orb-overlay: surface error: {e:?}");
                return;
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let quads = scene::build(&win.texture, &self.orb, self.scale, cw, ch, hover, shimmer, bob);
        let _ = gpu.draw(&view, cw, ch, &quads);
        frame.present();
    }

    /// A window moved. Persist it only when the move came from a user drag;
    /// otherwise just record where the window sits so a later drag measures right.
    fn on_moved(&mut self, pos: PhysicalPosition<i32>) {
        let Some(win) = self.win.as_mut() else {
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
        if !win.gesture.dragging() {
            return; // OS-initiated placement, not a user drag
        }
        win.last_move = Instant::now();
        let dv = DVec2::new(logical.x, logical.y);
        self.orb.pos = Some(dv);
        if let Err(e) = db::set_position(&self.db, dv) {
            eprintln!("xp-orb-overlay: save position failed: {e}");
        }
    }

    /// Move the orb to follow a two-finger trackpad scroll, persisting like a drag.
    /// A scroll has no button for `Window::drag_window` to own, so we move the
    /// window ourselves and persist only when the gesture settles (so a flick's
    /// momentum tail does not open a SQLite connection per frame). See
    /// [`overlay_core::scroll_drag_delta`].
    fn scroll_move(&mut self, delta: MouseScrollDelta, phase: TouchPhase) {
        let Some(win) = self.win.as_mut() else {
            return;
        };
        let (dx, dy) = overlay_core::scroll_drag_delta(delta, win.window.scale_factor());
        if (dx != 0.0 || dy != 0.0) && let Some(cur) = win.self_set {
            let np = LogicalPosition::new(cur.x + dx, cur.y + dy);
            win.self_set = Some(np);
            win.window.set_outer_position(np);
            win.last_move = Instant::now();
            self.orb.pos = Some(DVec2::new(np.x, np.y));
        }
        let settle = phase == TouchPhase::Ended || matches!(delta, MouseScrollDelta::LineDelta(..));
        if settle
            && let Some(pos) = win.self_set
            && let Err(e) = db::set_position(&self.db, DVec2::new(pos.x, pos.y))
        {
            eprintln!("xp-orb-overlay: save position failed: {e}");
        }
    }
}

impl ApplicationHandler<Orb> for App {
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

        self.orb = db::read_once(&self.db).unwrap_or_default();
        self.create_window(event_loop);

        let proxy = self.proxy.clone();
        db::spawn_watcher(self.db.clone(), move |orb| proxy.send_event(orb).is_ok());
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, orb: Orb) {
        self.orb = orb;
        // Honor an externally set position once the window has settled, skipping
        // the echo of our own drag/scroll.
        if let (Some(p), Some(win)) = (self.orb.pos, self.win.as_mut()) {
            let lp = LogicalPosition::new(p.x, p.y);
            let settled = Instant::now().duration_since(win.last_move) >= SETTLE;
            if settled && win.self_set != Some(lp) {
                win.window.set_outer_position(lp);
                win.self_set = Some(lp);
            }
        }
        if let Some(win) = self.win.as_ref() {
            win.window.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let (Some(gpu), Some(win)) = (self.gpu.as_ref(), self.win.as_mut()) {
                    win.config.width = size.width.max(1);
                    win.config.height = size.height.max(1);
                    win.surface.configure(gpu.device(), &win.config);
                }
                self.render();
            }
            WindowEvent::RedrawRequested => self.render(),
            WindowEvent::CursorEntered { .. } => {
                // winit's own tracking rect and the always-active NSTrackingArea
                // both deliver mouseEntered:, so this can arrive twice; act once.
                if let Some(win) = self.win.as_mut()
                    && !win.hovered
                {
                    win.hovered = true;
                    win.window.set_cursor(CursorIcon::Grab);
                    win.window.request_redraw();
                    ocwin::raise_to_front(&win.window);
                }
            }
            WindowEvent::CursorLeft { .. } => {
                if let Some(win) = self.win.as_mut() {
                    win.hovered = false;
                    win.window.set_cursor(CursorIcon::Default);
                    win.window.request_redraw();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let start_drag = self
                    .win
                    .as_mut()
                    .is_some_and(|win| win.gesture.cursor_moved(position));
                if start_drag
                    && let Some(win) = self.win.as_ref()
                {
                    win.window.set_cursor(CursorIcon::Grabbing);
                    if let Err(e) = win.window.drag_window() {
                        eprintln!("xp-orb-overlay: drag failed: {e}");
                    }
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if let Some(win) = self.win.as_mut() {
                    win.gesture.pressed();
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => {
                // A release where the press never became a drag is a click: open
                // the orb's URL if it has one.
                let clicked = if let Some(win) = self.win.as_mut() {
                    let c = win.gesture.released();
                    win.window.set_cursor(if win.hovered {
                        CursorIcon::Grab
                    } else {
                        CursorIcon::Default
                    });
                    c
                } else {
                    false
                };
                if clicked && !self.orb.url.trim().is_empty() {
                    open_url(&self.orb.url);
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Right,
                ..
            } => {
                // Right-click opens a native menu to dismiss the orb (its one
                // window, so closing it quits the overlay).
                if overlay_core::menu::popup(&["Close"]) == Some(0) {
                    event_loop.exit();
                }
            }
            WindowEvent::Moved(pos) => self.on_moved(pos),
            WindowEvent::MouseWheel { delta, phase, .. } => self.scroll_move(delta, phase),
            _ => {}
        }
    }

    /// The orb bobs and shimmers continuously, so keep frames coming at ~30 fps.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(win) = self.win.as_ref() {
            win.window.request_redraw();
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + FRAME));
    }
}

/// Run the experience-orb overlay event loop. Blocks until the window closes.
pub fn run(db: PathBuf, base_scale: u32) -> Result<(), Box<dyn std::error::Error>> {
    let event_loop: EventLoop<Orb> = ocwin::build_event_loop()?;
    let proxy = event_loop.create_proxy();
    let mut app = App {
        db,
        base_scale: base_scale.max(1),
        proxy,
        instance: wgpu::Instance::default(),
        gpu: None,
        win: None,
        orb: Orb::default(),
        mon_logical: (1920.0, 1080.0),
        scale_factor: 1.0,
        scale: base_scale.max(1),
        start: Instant::now(),
        ready: false,
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}
