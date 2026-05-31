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
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
/// Redraw cadence for a bar showing a live elapsed counter: once a second is
/// enough to advance a clock that ticks in seconds, and lets the loop sleep the
/// rest of the time.
const TICK: Duration = Duration::from_secs(1);
/// Pointer travel (physical px) past which a press becomes a drag rather than a
/// click. Below it, the press-release opens the bar's URL; at or above it, the
/// native window drag takes over.
const DRAG_THRESHOLD: f64 = 5.0;
/// How long a window must sit still after its last move before the overlay will
/// apply an externally-read position again. Must exceed the DB watch poll so the
/// watcher's lagged read-back of our own drag never snaps the window back.
const SETTLE: Duration = Duration::from_millis(700);

/// Ease-out cubic: fast start, soft landing. The default feedback curve.
fn ease_out_cubic(t: f32) -> f32 {
    let u = 1.0 - t.clamp(0.0, 1.0);
    1.0 - u * u * u
}

/// Current wall-clock time as Unix seconds, for the live elapsed counter. Before
/// the epoch (clock unset) it clamps to 0, which reads as a 0 counter.
fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Open a bar's click URL with the platform's opener (a URL, file, or any URI it
/// accepts), detached so the overlay never blocks on the browser launch. A
/// spawn failure is reported but not fatal: a bad URL should not take down the HUD.
fn open_url(url: &str) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    if let Err(e) = std::process::Command::new(opener).arg(url).spawn() {
        eprintln!("bossbar-overlay: failed to open {url}: {e}");
    }
}

/// Raise `window` above its same-level siblings without taking keyboard focus,
/// so a hovered bar's pop-down panel paints over the neighbouring bars instead
/// of slipping under them. Every bar is its own `WindowLevel::AlwaysOnTop`
/// window, and windows sharing one level stack by front-to-back order, so a bar
/// created earlier would otherwise expand beneath a later one.
///
/// `-[NSWindow orderFrontRegardless]` reorders the window without making it key,
/// so the user's keyboard focus stays where it was: a passive HUD must never
/// steal it, which rules out winit's `focus_window`. winit exposes no
/// non-activating raise, so reach the `NSWindow` through the raw AppKit handle.
#[cfg(target_os = "macos")]
fn raise_to_front(window: &Window) {
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
    // borrow stays valid (it is dropped before we return). We run inside the
    // winit event loop on the main thread, as these MainThreadOnly types require.
    let view: &NSView = unsafe { appkit.ns_view.cast().as_ref() };
    if let Some(ns_window) = view.window() {
        unsafe { ns_window.orderFrontRegardless() };
    }
}

/// Non-macOS: X11 and Wayland give an app no non-activating raise among
/// same-level always-on-top windows, so leave stacking to the compositor.
#[cfg(not(target_os = "macos"))]
fn raise_to_front(_window: &Window) {}

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
    /// True once a press has moved past the drag threshold and the native window
    /// drag has taken over. Drives the grabbing cursor; cleared on release.
    dragging: bool,
    /// Last position we know the window holds (logical points): what we set, or
    /// where the OS placed it. Lets `Moved` skip echoes of our own placement.
    self_set: Option<LogicalPosition<f64>>,
    /// This bar carries a description, so it has a hover pop-down panel.
    has_description: bool,
    /// The window is currently grown to fit the open panel. Collapsed otherwise,
    /// so the empty area below the bar never intercepts the mouse (click-through).
    expanded: bool,
    /// Last cursor position within the window (physical px), tracked so a press
    /// can measure drag distance and tell a click from a drag.
    cursor: Option<PhysicalPosition<f64>>,
    /// The left button is down. A release while this is set and the gesture never
    /// became a drag is a click. Independent of `press` so a press with no prior
    /// cursor sample still registers a click.
    pressing: bool,
    /// The anchor a drag's distance is measured from (cursor at press, or the
    /// first move after a press that had no cursor yet). `None` until anchored.
    press: Option<PhysicalPosition<f64>>,
    /// When the window last moved (a `Moved` during a drag). The reconcile guard
    /// ignores externally-read positions until the window has been still for
    /// SETTLE, so the watcher's lagged read-back never snaps a live drag back.
    last_move: Instant,
}

impl BarWin {
    /// Still moving: easing toward/away from hover, or breathing while hovered.
    fn animating(&self) -> bool {
        self.hovered || self.hover_anim > 0.0
    }

    /// The window should be grown for the panel while the bar is hovered or
    /// still easing back from a hover, but only when it has a description.
    fn wants_expanded(&self) -> bool {
        self.has_description && self.animating()
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
        let has_description = !bar.description.trim().is_empty();
        let now = Instant::now();
        Some(BarWin {
            window,
            surface,
            config,
            bar,
            hovered: false,
            hover_anim: 0.0,
            last: now,
            dragging: false,
            self_set: Some(pos),
            has_description,
            // Windows are created at the collapsed (bar-only) size; the panel
            // grows them on hover.
            expanded: false,
            cursor: None,
            pressing: false,
            press: None,
            // Far enough in the past that a fresh window accepts an external
            // position immediately (the SETTLE guard only fires after a real move).
            last_move: now - SETTLE,
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
                // A change to title presence or the description (while open)
                // changes the window's collapsed or expanded size.
                let size_affecting_changed = win.bar.title.is_empty() != b.title.is_empty()
                    || win.bar.description.trim().is_empty() != b.description.trim().is_empty()
                    || (win.expanded && win.bar.description != b.description);
                win.has_description = !b.description.trim().is_empty();
                win.bar = b.clone();
                // Honor a position set in the DB by something other than our own
                // drag (e.g. the CLI), but do not fight a live drag: while the
                // user is moving the window the DB still holds an older value (the
                // watcher reads it ~200ms behind), and forcing it would snap the
                // window back. So only apply an external position once the window
                // has been still for SETTLE, and skip the echo of our own move.
                if let Some(p) = b.pos {
                    let lp = LogicalPosition::new(p.x, p.y);
                    let settled = Instant::now().duration_since(win.last_move) >= SETTLE;
                    if settled && win.self_set != Some(lp) {
                        win.window.set_outer_position(lp);
                        win.self_set = Some(lp);
                    }
                }
                if size_affecting_changed {
                    // Reset to the resting size; if the bar is still hovered the
                    // settle pass in `about_to_wait` re-expands it to the new
                    // panel height on the next tick.
                    win.expanded = false;
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
            now_unix(),
            &win.bar,
            hover,
            breathe,
        );
        frame.present();
    }

    /// A window moved. Persist it only when the move came from a user drag;
    /// otherwise (initial placement, OS menu-bar clamp) just record where the
    /// window actually sits so a later drag is measured from the right spot.
    /// `Moved` arrives at roughly the refresh rate during a drag and the write is
    /// cheap (see `db::set_position`), so every drag move is saved; the last one
    /// is the exact drop position.
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
        // Mark the window as moving now, so the reconcile guard keeps the
        // watcher's lagged read-back from snapping it back mid-drag.
        win.last_move = Instant::now();
        let dv = DVec2::new(logical.x, logical.y);
        win.bar.pos = Some(dv);
        if let Err(e) = db::set_position(&self.db, id, dv) {
            eprintln!("bossbar-overlay: save position failed: {e}");
        }
    }

    /// Grow or shrink a bar's window to match its hover state. Expanding makes
    /// room for the description panel to unfold; collapsing restores the bar-only
    /// size so the area below the bar stops intercepting the mouse, keeping the
    /// desktop click-through everywhere off a bar. No-op when already in the
    /// target state or when the bar has no panel.
    fn set_window_expanded(&mut self, id: i64, expand: bool) {
        let Some(core) = self.core.as_mut() else {
            return;
        };
        let Some(win) = self.wins.get_mut(&id) else {
            return;
        };
        if win.expanded == expand || (expand && !win.has_description) {
            return;
        }
        let (w_px, h_px) = if expand {
            core.renderer.expanded_window_px(&win.bar)
        } else {
            render::bar_window_px(self.scale, !win.bar.title.is_empty())
        };
        win.expanded = expand;
        // Rely on the resulting `Resized` event to reconfigure the surface, the
        // same way a title-presence change in `reconcile` does.
        let _ = win.window.request_inner_size(PhysicalSize::new(w_px, h_px));
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
                    // Bring this bar above its siblings so its pop-down panel
                    // covers them instead of unfolding underneath.
                    raise_to_front(&win.window);
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
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(win) = self.wins.get_mut(&id) {
                    win.cursor = Some(position);
                    // While the button is down and not yet dragging, measure travel
                    // from the anchor (the cursor at press, or this first move if
                    // the press had no cursor sample). Past the threshold it becomes
                    // a drag and hands off to the native window drag, which then
                    // drives `Moved`.
                    if win.pressing && !win.dragging {
                        let origin = *win.press.get_or_insert(position);
                        let dx = position.x - origin.x;
                        let dy = position.y - origin.y;
                        if (dx * dx + dy * dy).sqrt() >= DRAG_THRESHOLD {
                            win.dragging = true;
                            win.window.set_cursor(CursorIcon::Grabbing);
                            if let Err(e) = win.window.drag_window() {
                                eprintln!("bossbar-overlay: drag failed: {e}");
                            }
                        }
                    }
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if let Some(win) = self.wins.get_mut(&id) {
                    // Defer the native drag until the pointer travels past the
                    // threshold (see CursorMoved), so a stationary press stays a
                    // click whose mouse-up we actually receive.
                    win.pressing = true;
                    win.press = win.cursor;
                    win.dragging = false;
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => {
                // A release where the button-down gesture never became a drag is a
                // click: open the bar's URL if it has one. (A drag swallows its own
                // mouse-up on macOS, but `dragging` is already set by then.)
                let url = self.wins.get_mut(&id).and_then(|win| {
                    let clicked = win.pressing && !win.dragging;
                    win.pressing = false;
                    win.press = None;
                    win.dragging = false;
                    win.window.set_cursor(if win.hovered {
                        CursorIcon::Grab
                    } else {
                        CursorIcon::Default
                    });
                    (clicked && !win.bar.url.trim().is_empty()).then(|| win.bar.url.clone())
                });
                if let Some(url) = url {
                    open_url(&url);
                }
            }
            WindowEvent::Moved(pos) => self.on_moved(id, pos),
            _ => {}
        }
    }

    /// Keep redrawing while any bar is animating (growing, shrinking, or
    /// breathing), capped to ~60 fps; tick once a second while any bar shows a
    /// live elapsed counter so its clock advances; otherwise let the loop sleep
    /// until the next event so an idle overlay costs nothing.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Settle each window's size to its hover state first: grow for an open
        // panel, shrink back once the hover has fully eased out. Done here, off
        // the per-frame easing, so the window resizes only twice per hover.
        let ids: Vec<i64> = self.wins.keys().copied().collect();
        for id in ids {
            let want = self.wins.get(&id).is_some_and(BarWin::wants_expanded);
            self.set_window_expanded(id, want);
        }

        let mut animating = false;
        let mut ticking = false;
        for win in self.wins.values() {
            // A live counter only needs the once-a-second TICK, but anything
            // mid-animation also wants its redraw now (the faster cadence wins).
            if win.animating() {
                animating = true;
                win.window.request_redraw();
            } else if win.bar.since.is_some() {
                ticking = true;
                win.window.request_redraw();
            }
        }
        event_loop.set_control_flow(if animating {
            ControlFlow::WaitUntil(Instant::now() + FRAME)
        } else if ticking {
            ControlFlow::WaitUntil(Instant::now() + TICK)
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
