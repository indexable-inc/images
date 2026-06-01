//! The live book overlay: one transparent, always-on-top, borderless window
//! holding a two-page spread. winit owns the window and event loop; the SQLite
//! watcher runs on its own thread and wakes the loop with a fresh [`Book`] on any
//! DB change.
//!
//! Off the window the desktop is click-through (there is no window there). On the
//! window: hovering grabs focus order so the book sits on top; dragging moves it
//! (the OS owns the drag via `Window::drag_window`, the position is read back and
//! persisted); a two-finger trackpad scroll also moves it (no button to hand to
//! `drag_window`, so the overlay moves the window itself via
//! [`overlay_core::scroll_drag_delta`]); a click on a page-turn arrow flips the
//! spread.

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
use overlay_core::{window as ocwin, DragClick, Gpu, HoverAnim};

use crate::book::Book;
use crate::db;
use crate::scene::{self, BookTextures};

/// Pointer travel (physical px) past which a press becomes a window drag.
const DRAG_THRESHOLD: f64 = 5.0;
/// How long the window must sit still after its last move before an externally
/// read position is applied again, so the watcher's lagged read-back of our own
/// drag never snaps it back.
const SETTLE: Duration = Duration::from_millis(700);
/// Hover grow/shrink time: an ease-out tween at the responsive end of the
/// feedback range, matching the boss bar. The hover animates in over this time
/// and then holds still so the page stays readable. See the `animation` skill.
const GROW: Duration = Duration::from_millis(160);
/// Largest animation step a single frame may apply, so a stall (the loop slept)
/// does not jump the hover; frames are otherwise ~16ms.
const MAX_STEP: Duration = Duration::from_millis(50);
/// Animation frame budget while something is moving (~60 fps).
const FRAME: Duration = Duration::from_millis(16);

/// Which page-turn arrow the pointer is over, if any.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Arrow {
    Back,
    Fwd,
}

struct WinState {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    textures: BookTextures,
    gesture: DragClick,
    /// The pointer is on the window: drives the whole-book grow + breathe.
    hovered: bool,
    /// Hover amounts advanced toward their targets each frame. `book` follows
    /// `hovered`; `back`/`fwd` follow whether the pointer is over that arrow. The
    /// scene module applies the easing curves to their raw values.
    book_anim: HoverAnim,
    back_anim: HoverAnim,
    fwd_anim: HoverAnim,
    /// Which arrow the pointer is currently over (resting space), for the cursor
    /// icon and to drive the per-arrow hover.
    over_arrow: Option<Arrow>,
    /// Last pointer position (physical px), for per-frame arrow hit-testing.
    cursor: Option<PhysicalPosition<f64>>,
    /// Timestamp of the last animation step, for frame-rate-independent easing.
    last: Instant,
    /// Last position we know the window holds (logical points): what we set, or
    /// where the OS placed it. Lets `Moved` skip echoes of our own placement.
    self_set: Option<LogicalPosition<f64>>,
    /// When the window last moved during a drag, for the reconcile settle guard.
    last_move: Instant,
}

impl WinState {
    /// Still mid-transition (the grow or an arrow easing in/out). There is no
    /// perpetual motion, so once every hover has settled to its target this is
    /// false and the loop sleeps, leaving the page still to read.
    fn animating(&self) -> bool {
        self.book_anim.is_animating() || self.back_anim.is_animating() || self.fwd_anim.is_animating()
    }
}

pub struct App {
    db: PathBuf,
    base_scale: u32,
    proxy: EventLoopProxy<Book>,
    instance: wgpu::Instance,
    gpu: Option<Gpu>,
    win: Option<WinState>,
    book: Book,
    /// Index of the left page of the visible spread (always even).
    spread: usize,
    mon_logical: (f64, f64),
    scale_factor: f64,
    /// Physical sprite scale, `round(base_scale * scale_factor)`.
    scale: u32,
    ready: bool,
}

/// Which page-turn arrow (if any) sits under the pointer at `c` (physical px).
/// The whole-book grow displaces the arrows from their resting layout, so the
/// cursor is mapped back through `mul` about the window centre before testing the
/// resting arrow rects. The rects are the stable hit region; the visual pop grows
/// past them without moving their centres, so a pointer inside never oscillates.
fn arrow_under(
    c: PhysicalPosition<f64>,
    mul: f32,
    scale: u32,
    win_w: u32,
    win_h: u32,
    show_back: bool,
    show_fwd: bool,
) -> Option<Arrow> {
    let cx = win_w as f64 * 0.5;
    let cy = win_h as f64 * 0.5;
    let m = mul as f64;
    let px = cx + (c.x - cx) / m;
    let py = cy + (c.y - cy) / m;
    let hit = |r: (f32, f32, f32, f32)| {
        let (x, y, rw, rh) = r;
        px >= x as f64 && px <= (x + rw) as f64 && py >= y as f64 && py <= (y + rh) as f64
    };
    if show_fwd && hit(scene::fwd_arrow_rect(scale, win_w, win_h)) {
        Some(Arrow::Fwd)
    } else if show_back && hit(scene::back_arrow_rect(scale, win_w, win_h)) {
        Some(Arrow::Back)
    } else {
        None
    }
}

impl App {
    /// Auto-centered window position (logical points) for the spread, within the
    /// screen's usable area so the book and its bottom page-turn arrows clear the
    /// menu bar and Dock. Falls back to the full display if the visible frame is
    /// unavailable. When the spread is larger than the usable area (a small
    /// display at a high scale) the offset clamps to zero, pinning it just below
    /// the menu bar rather than centering it under the bar.
    fn center_pos(&self) -> LogicalPosition<f64> {
        let (w_px, h_px) = scene::spread_window_px(self.scale);
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
        let (w_px, h_px) = scene::spread_window_px(self.scale);
        let pos = self
            .book
            .pos
            .map(|p| LogicalPosition::new(p.x, p.y))
            .unwrap_or_else(|| self.center_pos());
        let attrs = ocwin::float_attributes("Book", w_px, h_px, Some(pos));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("book-overlay: create window failed: {e}");
                event_loop.exit();
                return;
            }
        };
        // The book is an accessory (background) window, so hover only reaches it
        // with an always-active tracking area; without this the page-turn arrows
        // never highlight while another app is focused.
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
        let textures = scene::register(&mut gpu);

        let size = window.inner_size();
        let config = ocwin::surface_config(format, alpha, size.width, size.height);
        surface.configure(gpu.device(), &config);
        window.request_redraw();

        self.gpu = Some(gpu);
        self.win = Some(WinState {
            window,
            surface,
            config,
            textures,
            gesture: DragClick::new(DRAG_THRESHOLD),
            hovered: false,
            book_anim: HoverAnim::default(),
            back_anim: HoverAnim::default(),
            fwd_anim: HoverAnim::default(),
            over_arrow: None,
            cursor: None,
            last: Instant::now(),
            self_set: Some(pos),
            last_move: Instant::now() - SETTLE,
        });
    }

    fn render(&mut self) {
        let now = Instant::now();
        let show_back = self.spread > 0;
        let show_fwd = self.spread + 2 <= self.book.last_spread();
        let scale = self.scale;

        // Advance the hover amounts toward their targets (disjoint field borrows:
        // `win` and `gpu`/`book`/`spread` are separate fields).
        let Some(win) = self.win.as_mut() else {
            return;
        };
        let (cw, ch) = (win.config.width, win.config.height);
        let dt = now.duration_since(win.last).min(MAX_STEP);
        win.last = now;
        win.book_anim
            .approach(if win.hovered { 1.0 } else { 0.0 }, dt, GROW);

        // Which arrow is the pointer over, accounting for the current whole-book
        // grow so the target tracks the arrow where it is actually drawn.
        let mul = scene::book_mul(win.book_anim.raw());
        let over = win
            .cursor
            .and_then(|c| arrow_under(c, mul, scale, cw, ch, show_back, show_fwd));
        win.back_anim
            .approach(if over == Some(Arrow::Back) { 1.0 } else { 0.0 }, dt, GROW);
        win.fwd_anim
            .approach(if over == Some(Arrow::Fwd) { 1.0 } else { 0.0 }, dt, GROW);

        // A pointer cursor over a live arrow signals it is clickable; otherwise the
        // grab/default icon for the draggable book. Skip while the OS owns a drag.
        if !win.gesture.dragging() && over != win.over_arrow {
            let icon = if over.is_some() {
                CursorIcon::Pointer
            } else if win.hovered {
                CursorIcon::Grab
            } else {
                CursorIcon::Default
            };
            win.window.set_cursor(icon);
        }
        win.over_arrow = over;

        let hover = scene::Hover {
            book: win.book_anim.raw(),
            back: win.back_anim.raw(),
            fwd: win.fwd_anim.raw(),
        };

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
                eprintln!("book-overlay: surface error: {e:?}");
                return;
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let quads = scene::build(
            gpu,
            &win.textures,
            &self.book,
            self.spread,
            scale,
            cw,
            ch,
            show_back,
            show_fwd,
            &hover,
        );
        let _ = gpu.draw(&view, cw, ch, &quads);
        frame.present();
    }

    /// A click landed at `pos` (physical px); flip the spread if it hit an arrow.
    fn on_click(&mut self, pos: PhysicalPosition<f64>) {
        let show_back = self.spread > 0;
        let show_fwd = self.spread + 2 <= self.book.last_spread();
        let scale = self.scale;
        let Some(win) = self.win.as_ref() else {
            return;
        };
        let mul = scene::book_mul(win.book_anim.raw());
        let (w, h) = (win.config.width, win.config.height);
        let window = win.window.clone();
        match arrow_under(pos, mul, scale, w, h, show_back, show_fwd) {
            Some(Arrow::Back) => {
                self.spread = self.spread.saturating_sub(2);
                crate::sound::page_flip();
                window.request_redraw();
            }
            Some(Arrow::Fwd) => {
                self.spread = (self.spread + 2).min(self.book.last_spread());
                crate::sound::page_flip();
                window.request_redraw();
            }
            None => {}
        }
    }

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
        self.book.pos = Some(dv);
        if let Err(e) = db::set_position(&self.db, dv) {
            eprintln!("book-overlay: save position failed: {e}");
        }
    }

    /// Move the book to follow a two-finger trackpad scroll, persisting the new
    /// position like a drag. There is no button for `Window::drag_window` to own,
    /// so we move the window ourselves: update `self_set` before the move so the
    /// resulting `Moved` reads as our own echo (no double write), refresh
    /// `last_move` so the settle guard does not snap it back from the watcher's
    /// lagged read, and write the position straight to the DB.
    fn scroll_move(&mut self, delta: MouseScrollDelta, phase: TouchPhase) {
        let Some(win) = self.win.as_mut() else {
            return;
        };
        let (dx, dy) = overlay_core::scroll_drag_delta(delta, win.window.scale_factor());
        // Move the window live on every event, momentum tail included, so it feels
        // like scrolling. `self_set` tracks where the window sits and is set after
        // create; measure the scroll from there.
        if (dx != 0.0 || dy != 0.0) && let Some(cur) = win.self_set {
            let np = LogicalPosition::new(cur.x + dx, cur.y + dy);
            win.self_set = Some(np);
            // Move the window AND warp the pointer with it, so the pointer stays on
            // the book like a press-drag rather than the book sliding out from under it.
            ocwin::move_window_with_cursor(&win.window, np, win.gesture.cursor());
            win.last_move = Instant::now();
            self.book.pos = Some(DVec2::new(np.x, np.y));
        }
        // Persist only when the gesture settles, not per frame: a trackpad flick
        // emits a long momentum tail of `MouseWheel` events, and writing on each
        // would open a SQLite connection per frame on the UI thread. The touch and
        // momentum ends both carry `TouchPhase::Ended`; a discrete wheel notch
        // (`LineDelta`) has no Ended phase but is low-frequency, so save it directly.
        let settle = phase == TouchPhase::Ended || matches!(delta, MouseScrollDelta::LineDelta(..));
        if settle
            && let Some(pos) = win.self_set
            && let Err(e) = db::set_position(&self.db, DVec2::new(pos.x, pos.y))
        {
            eprintln!("book-overlay: save position failed: {e}");
        }
    }
}

impl ApplicationHandler<Book> for App {
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

        self.book = db::read_once(&self.db).unwrap_or_else(|_| Book {
            pages: vec![String::new()],
            pos: None,
        });
        self.spread = self.spread.min(self.book.last_spread());
        self.create_window(event_loop);

        let proxy = self.proxy.clone();
        db::spawn_watcher(self.db.clone(), move |book| proxy.send_event(book).is_ok());
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, book: Book) {
        self.book = book;
        self.spread = self.spread.min(self.book.last_spread());
        // Honor an externally set position once the window has settled, skipping
        // the echo of our own drag.
        if let (Some(p), Some(win)) = (self.book.pos, self.win.as_mut()) {
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
                // (enable_background_hover) both deliver mouseEntered:, so this can
                // arrive twice per crossing; act only on the first to avoid a
                // redundant raise/redraw.
                if let Some(win) = self.win.as_mut() {
                    if !win.hovered {
                        win.hovered = true;
                        win.window.set_cursor(CursorIcon::Grab);
                        win.window.request_redraw();
                        ocwin::raise_to_front(&win.window);
                    }
                }
            }
            WindowEvent::CursorLeft { .. } => {
                if let Some(win) = self.win.as_mut() {
                    win.hovered = false;
                    win.cursor = None;
                    win.over_arrow = None;
                    win.window.set_cursor(CursorIcon::Default);
                    win.window.request_redraw();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let start_drag = self.win.as_mut().is_some_and(|win| {
                    win.cursor = Some(position);
                    // Redraw so the arrow under the pointer updates promptly; while
                    // animating `about_to_wait` already keeps frames coming.
                    win.window.request_redraw();
                    win.gesture.cursor_moved(position)
                });
                if start_drag {
                    if let Some(win) = self.win.as_ref() {
                        win.window.set_cursor(CursorIcon::Grabbing);
                        if let Err(e) = win.window.drag_window() {
                            eprintln!("book-overlay: drag failed: {e}");
                        }
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
                let click = self.win.as_mut().and_then(|win| {
                    let clicked = win.gesture.released();
                    win.window.set_cursor(if win.hovered {
                        CursorIcon::Grab
                    } else {
                        CursorIcon::Default
                    });
                    clicked.then(|| win.gesture.cursor()).flatten()
                });
                if let Some(pos) = click {
                    self.on_click(pos);
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Right,
                ..
            } => {
                // Right-click opens a native menu to dismiss the book (its one
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

    /// Keep redrawing at ~60 fps while the book is animating (growing, shrinking,
    /// breathing, or an arrow highlighting); otherwise let the loop sleep until the
    /// next event so an idle overlay costs nothing.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let animating = self.win.as_ref().is_some_and(WinState::animating);
        if animating {
            if let Some(win) = self.win.as_ref() {
                win.window.request_redraw();
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + FRAME));
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }
}

/// Run the book overlay event loop. Blocks until the window closes.
pub fn run(db: PathBuf, base_scale: u32) -> Result<(), Box<dyn std::error::Error>> {
    let event_loop: EventLoop<Book> = ocwin::build_event_loop()?;
    let proxy = event_loop.create_proxy();
    let mut app = App {
        db,
        base_scale: base_scale.max(1),
        proxy,
        instance: wgpu::Instance::default(),
        gpu: None,
        win: None,
        book: Book {
            pages: vec![String::new()],
            pos: None,
        },
        spread: 0,
        mon_logical: (1920.0, 1080.0),
        scale_factor: 1.0,
        scale: base_scale.max(1),
        ready: false,
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}
