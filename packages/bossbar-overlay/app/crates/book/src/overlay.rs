//! The live book overlay: one transparent, always-on-top, borderless window
//! holding a two-page spread. winit owns the window and event loop; the SQLite
//! watcher runs on its own thread and wakes the loop with a fresh [`Book`] on any
//! DB change.
//!
//! Off the window the desktop is click-through (there is no window there). On the
//! window: hovering grabs focus order so the book sits on top; dragging moves it
//! (the OS owns the drag via `Window::drag_window`, the position is read back and
//! persisted); a click on a page-turn arrow flips the spread.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use overlay_core::glam::DVec2;
use overlay_core::wgpu;
use overlay_core::winit::application::ApplicationHandler;
use overlay_core::winit::dpi::{LogicalPosition, PhysicalPosition};
use overlay_core::winit::event::{ElementState, MouseButton, WindowEvent};
use overlay_core::winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use overlay_core::winit::window::{CursorIcon, Window, WindowId};
use overlay_core::{window as ocwin, DragClick, Gpu};

use crate::book::Book;
use crate::db;
use crate::scene::{self, BookTextures};

/// Pointer travel (physical px) past which a press becomes a window drag.
const DRAG_THRESHOLD: f64 = 5.0;
/// How long the window must sit still after its last move before an externally
/// read position is applied again, so the watcher's lagged read-back of our own
/// drag never snaps it back.
const SETTLE: Duration = Duration::from_millis(700);

struct WinState {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    textures: BookTextures,
    gesture: DragClick,
    hovered: bool,
    /// Last position we know the window holds (logical points): what we set, or
    /// where the OS placed it. Lets `Moved` skip echoes of our own placement.
    self_set: Option<LogicalPosition<f64>>,
    /// When the window last moved during a drag, for the reconcile settle guard.
    last_move: Instant,
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

impl App {
    fn show_back(&self) -> bool {
        self.spread > 0
    }

    fn show_fwd(&self) -> bool {
        self.spread + 2 <= self.book.last_spread()
    }

    /// Auto-centered window position (logical points) for the spread.
    fn center_pos(&self) -> LogicalPosition<f64> {
        let (w_px, h_px) = scene::spread_window_px(self.scale);
        let wl = w_px as f64 / self.scale_factor;
        let hl = h_px as f64 / self.scale_factor;
        LogicalPosition::new(
            ((self.mon_logical.0 - wl) * 0.5).max(0.0),
            ((self.mon_logical.1 - hl) * 0.5).max(0.0),
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
            self_set: Some(pos),
            last_move: Instant::now() - SETTLE,
        });
    }

    fn render(&mut self) {
        let (Some(gpu), Some(win)) = (self.gpu.as_ref(), self.win.as_mut()) else {
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
            self.scale,
            win.config.width,
            win.config.height,
            self.spread > 0,
            self.spread + 2 <= self.book.last_spread(),
        );
        let _ = gpu.draw(&view, win.config.width, win.config.height, &quads);
        frame.present();
    }

    /// A click landed at `pos` (physical px); flip the spread if it hit an arrow.
    fn on_click(&mut self, pos: PhysicalPosition<f64>) {
        let Some(win) = self.win.as_ref() else {
            return;
        };
        let (w, h) = (win.config.width, win.config.height);
        let hit = |r: (f32, f32, f32, f32)| {
            let (x, y, rw, rh) = r;
            pos.x as f32 >= x
                && pos.x as f32 <= x + rw
                && pos.y as f32 >= y
                && pos.y as f32 <= y + rh
        };
        if self.show_back() && hit(scene::back_arrow_rect(self.scale, w, h)) {
            self.spread = self.spread.saturating_sub(2);
            self.win.as_ref().unwrap().window.request_redraw();
        } else if self.show_fwd() && hit(scene::fwd_arrow_rect(self.scale, w, h)) {
            self.spread = (self.spread + 2).min(self.book.last_spread());
            self.win.as_ref().unwrap().window.request_redraw();
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
                if let Some(win) = self.win.as_ref() {
                    win.window.set_cursor(CursorIcon::Grab);
                    ocwin::raise_to_front(&win.window);
                }
                if let Some(win) = self.win.as_mut() {
                    win.hovered = true;
                }
            }
            WindowEvent::CursorLeft { .. } => {
                if let Some(win) = self.win.as_mut() {
                    win.hovered = false;
                    win.window.set_cursor(CursorIcon::Default);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let start_drag = self
                    .win
                    .as_mut()
                    .is_some_and(|win| win.gesture.cursor_moved(position));
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
            WindowEvent::Moved(pos) => self.on_moved(pos),
            _ => {}
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
