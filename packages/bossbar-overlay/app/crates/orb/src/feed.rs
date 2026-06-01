//! The merge "feed" overlay: a full-screen, transparent, always-on-top,
//! click-through window that watches the `events` table and, for each newly
//! queued row, floats a labelled Minecraft experience orb up from the bottom of
//! the screen and fades it out ("rise & pop"), like collecting XP. Driven the
//! same way as the pinned orb: anyone writes a row
//! (`xp-orb-overlay push "ix · Fix flaky test"`) and it animates within ~200ms.
//!
//! Unlike the pinned orb this window spans the whole visible frame and is fully
//! click-through (`set_cursor_hittest(false)`), so it never intercepts the
//! desktop: it is pure output. Concurrent announcements stack into vertical
//! slots so they never overlap, each rising and fading on its own clock.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use overlay_core::wgpu;
use overlay_core::winit::application::ApplicationHandler;
use overlay_core::winit::dpi::{LogicalPosition, PhysicalSize};
use overlay_core::winit::event::WindowEvent;
use overlay_core::winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use overlay_core::winit::window::{Window, WindowId};
use overlay_core::{Gpu, anim, window as ocwin};

use crate::db;
use crate::scene::{self, OrbTexture};

/// How long one announcement orb lives, start to fully faded.
const LIFESPAN: Duration = Duration::from_millis(4500);
/// Fraction of the life spent fully opaque before the fade-out begins.
const FADE_FROM: f32 = 0.55;
/// Logical points an orb floats up over its life.
const RISE: f64 = 90.0;
/// Logical points from the bottom of the visible frame to the lowest slot,
/// clearing the Dock.
const BOTTOM_MARGIN: f64 = 64.0;
/// Logical points from the left edge to the orb.
const LEFT_MARGIN: f64 = 40.0;
/// Vertical gap between stacked slots, as a multiple of the orb sprite height.
const ROW_MUL: f64 = 1.4;
/// DB poll cadence: a pushed event animates within this.
const POLL: Duration = Duration::from_millis(200);
/// Animation frame budget while at least one orb is on screen (~30 fps).
const FRAME: Duration = Duration::from_millis(33);
/// One full shimmer (colour pulse) cycle, matching the pinned orb.
const SHIMMER_PERIOD: Duration = Duration::from_millis(2000);
/// Consumed events older than this are pruned so the table stays small.
const PRUNE_AGE_SECS: i64 = 300;
/// Cap on the stacking slot used for vertical placement, so a rare burst of
/// simultaneous merges overlaps near the top of the stack instead of marching
/// off the top of the screen. Slots still allocate beyond this; only the drawn
/// height is clamped.
const MAX_SLOT: usize = 9;

/// One in-flight announcement: a labelled orb rising in a vertical slot.
struct Pop {
    text: String,
    amount: i64,
    born: Instant,
    /// Vertical stacking position (0 = lowest), assigned at spawn from the free
    /// slots so concurrent pops never overlap.
    slot: usize,
}

struct WinState {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    texture: OrbTexture,
}

pub struct Feed {
    db: PathBuf,
    base_scale: u32,
    instance: wgpu::Instance,
    gpu: Option<Gpu>,
    win: Option<WinState>,
    /// Open reader connection; reopened lazily on error.
    conn: Option<rusqlite::Connection>,
    /// Highest event id already consumed; seeded to the current max on startup so
    /// the existing backlog stays quiet and only new pushes animate.
    last_seen: i64,
    pops: Vec<Pop>,
    last_poll: Instant,
    last_prune: Instant,
    scale_factor: f64,
    /// Physical sprite scale, `round(base_scale * scale_factor)`.
    scale: u32,
    start: Instant,
    ready: bool,
}

impl Feed {
    fn create_window(&mut self, event_loop: &ActiveEventLoop) {
        let (left, top, vw, vh) =
            ocwin::visible_frame_logical().unwrap_or((0.0, 0.0, 1920.0, 1080.0));
        let w_px = (vw * self.scale_factor).round().max(1.0) as u32;
        let h_px = (vh * self.scale_factor).round().max(1.0) as u32;
        let pos = LogicalPosition::new(left, top);
        let attrs = ocwin::float_attributes("Merge Feed", w_px, h_px, Some(pos))
            .with_inner_size(PhysicalSize::new(w_px, h_px));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("xp-orb-overlay: create feed window failed: {e}");
                event_loop.exit();
                return;
            }
        };
        // Pure output: never intercept the desktop. The whole window is
        // click-through, so pointer events fall to whatever is behind it.
        let _ = window.set_cursor_hittest(false);
        ocwin::raise_to_front(&window);

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

        self.gpu = Some(gpu);
        self.win = Some(WinState {
            window,
            surface,
            config,
            texture,
        });
    }

    /// Poll the DB for events queued since `last_seen`, spawning a pop for each.
    fn poll_events(&mut self, now: Instant) {
        if self.conn.is_none() {
            self.conn = db::connect(&self.db).ok();
            if let Some(c) = self.conn.as_ref()
                && self.last_seen == 0
            {
                // Seed the cursor only on the FIRST successful connect, so the
                // pre-existing backlog stays quiet. On a reconnect after a read
                // error, keep the prior cursor so events queued during the
                // outage are still picked up rather than silently skipped.
                self.last_seen = db::max_event_id(c).unwrap_or(0);
            }
        }
        let Some(conn) = self.conn.as_ref() else {
            return;
        };
        match db::read_events_after(conn, self.last_seen) {
            Ok(events) => {
                for ev in events {
                    self.last_seen = self.last_seen.max(ev.id);
                    let slot = free_slot(&self.pops);
                    self.pops.push(Pop {
                        text: ev.text,
                        amount: ev.amount,
                        born: now,
                        slot,
                    });
                }
            }
            Err(e) => {
                eprintln!("xp-orb-overlay: feed read failed, reopening: {e}");
                self.conn = None;
            }
        }

        if now.duration_since(self.last_prune) >= Duration::from_secs(60) {
            if let Some(c) = self.conn.as_ref() {
                let _ = db::prune_events(c, PRUNE_AGE_SECS);
            }
            self.last_prune = now;
        }
    }

    fn render(&mut self) {
        let now = Instant::now();
        let shimmer = (anim::breathe(now.duration_since(self.start), SHIMMER_PERIOD) + 1.0) * 0.5;
        let (Some(gpu), Some(win)) = (self.gpu.as_ref(), self.win.as_ref()) else {
            return;
        };
        let (cw, ch) = (win.config.width, win.config.height);

        let orb_px = (16 * self.scale) as f32; // ORB_PX * scale
        let row = orb_px as f64 * ROW_MUL;
        let rise_px = RISE * self.scale_factor;
        let left_px = LEFT_MARGIN * self.scale_factor;
        let bottom_px = BOTTOM_MARGIN * self.scale_factor;

        let mut quads = Vec::new();
        for pop in &self.pops {
            let p = (now.duration_since(pop.born).as_secs_f32() / LIFESPAN.as_secs_f32())
                .clamp(0.0, 1.0);
            let alpha = if p < FADE_FROM {
                1.0
            } else {
                1.0 - (p - FADE_FROM) / (1.0 - FADE_FROM)
            };
            let base_y =
                ch as f64 - bottom_px - orb_px as f64 - pop.slot.min(MAX_SLOT) as f64 * row;
            let y = base_y - rise_px * anim::ease_out_cubic(p) as f64;
            scene::build_pop(
                gpu,
                &win.texture,
                &pop.text,
                pop.amount,
                self.scale,
                left_px as f32,
                y as f32,
                alpha,
                shimmer,
                &mut quads,
            );
        }

        let frame = match win.surface.get_current_texture() {
            Ok(f) => f,
            Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                win.surface.configure(gpu.device(), &win.config);
                return;
            }
            Err(e) => {
                eprintln!("xp-orb-overlay: feed surface error: {e:?}");
                return;
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let _ = gpu.draw(&view, cw, ch, &quads);
        frame.present();
    }
}

/// Lowest non-negative slot not held by an active pop, so a new orb stacks above
/// the current ones without overlapping and reuses a slot freed by a faded orb.
fn free_slot(pops: &[Pop]) -> usize {
    let used: HashSet<usize> = pops.iter().map(|p| p.slot).collect();
    (0..).find(|s| !used.contains(s)).unwrap_or(0)
}

impl ApplicationHandler<()> for Feed {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.ready {
            return;
        }
        let scale_factor = event_loop
            .primary_monitor()
            .or_else(|| event_loop.available_monitors().next())
            .map_or(1.0, |m| m.scale_factor());
        self.scale_factor = scale_factor;
        self.scale = ((self.base_scale as f64) * scale_factor).round().max(1.0) as u32;
        self.ready = true;
        self.create_window(event_loop);
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
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        if now.duration_since(self.last_poll) >= POLL {
            self.poll_events(now);
            self.last_poll = now;
        }
        // Drop faded orbs; an emptying transition still needs one redraw to clear.
        let before = self.pops.len();
        self.pops.retain(|p| now.duration_since(p.born) < LIFESPAN);
        let changed = self.pops.len() != before;
        let active = !self.pops.is_empty();
        if (active || changed)
            && let Some(win) = self.win.as_ref()
        {
            win.window.request_redraw();
        }
        let next = if active { FRAME } else { POLL };
        event_loop.set_control_flow(ControlFlow::WaitUntil(now + next));
    }
}

/// Run the merge-feed overlay event loop. Blocks until the window closes.
pub fn run(db: PathBuf, base_scale: u32) -> Result<(), Box<dyn std::error::Error>> {
    let event_loop: EventLoop<()> = ocwin::build_event_loop()?;
    let now = Instant::now();
    let mut feed = Feed {
        db,
        base_scale: base_scale.max(1),
        instance: wgpu::Instance::default(),
        gpu: None,
        win: None,
        conn: None,
        last_seen: 0,
        pops: Vec::new(),
        last_poll: now - POLL,
        last_prune: now,
        scale_factor: 1.0,
        scale: base_scale.max(1),
        start: now,
        ready: false,
    };
    event_loop.run_app(&mut feed)?;
    Ok(())
}
