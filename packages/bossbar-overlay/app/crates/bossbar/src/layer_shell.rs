//! Wayland layer-shell backend for the live boss bar overlay.
//!
//! Normal Wayland toplevel windows cannot choose their own global position:
//! compositors such as Hyprland are free to center new floating windows and ignore
//! winit's requested top-left. Layer-shell is the compositor-owned protocol for
//! panels and overlays, so each boss bar is represented as a small top-layer
//! surface anchored to the top of the output. Auto-stacked bars use a top-only
//! anchor, letting the compositor keep them horizontally centered; when a bar is
//! dragged, the xdg-output size gives us the centered left edge before switching
//! it to top-left anchoring with persisted margins.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use layershellev::calloop::channel;
use layershellev::id::Id as ShellId;
// layershellev re-exports wl_pointer at this level; it does not expose
// wayland_client::protocol through its wrapper module.
use layershellev::reexport::wayland_client::WEnum;
use layershellev::reexport::wayland_client::wl_pointer::ButtonState;
use layershellev::reexport::{Anchor, KeyboardInteractivity, Layer};
use layershellev::{
    AxisScroll, DispatchMessage, LayerShellEvent, NewLayerShellSettings, OutputOption,
    RefreshRequest, ReturnData, WindowState,
};
use overlay_core::glam::DVec2;
use overlay_core::wgpu;
use overlay_core::winit::dpi::PhysicalPosition;
use overlay_core::{DragClick, Gpu, HoverAnim, TexHandle, anim, window as ocwin};

use crate::bars::BossBar;
use crate::db;
use crate::scene::{self, BarTextures};

const AUTO_TOP: f64 = 40.0;
const AUTO_GAP: f64 = 6.0;
const GROW: Duration = Duration::from_millis(160);
const BREATHE_PERIOD: Duration = Duration::from_millis(2600);
const MAX_STEP: Duration = Duration::from_millis(50);
const FRAME: Duration = Duration::from_millis(16);
const TICK: Duration = Duration::from_secs(1);
const SETTLE: Duration = Duration::from_millis(700);
const SCROLL_SAVE_AFTER: Duration = Duration::from_millis(150);
const DRAG_THRESHOLD: f64 = 5.0;
const LINE_POINTS: f64 = 16.0;
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Pos {
    x: f64,
    y: f64,
}

impl Pos {
    fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

struct GpuCore {
    gpu: Gpu,
    textures: BarTextures,
    format: wgpu::TextureFormat,
    alpha_mode: wgpu::CompositeAlphaMode,
    /// Icon path -> uploaded texture, memoized so a bar's avatar uploads once.
    /// `None` records a path that failed to load, so it is tried once then skipped.
    icon_cache: HashMap<String, Option<TexHandle>>,
}

impl GpuCore {
    /// Resolve an icon path to its texture, loading and caching on first use.
    fn icon(&mut self, path: &str) -> Option<TexHandle> {
        if path.is_empty() {
            return None;
        }
        if let Some(cached) = self.icon_cache.get(path) {
            return *cached;
        }
        // Read failure is transient (writer may not have created the file yet) so
        // is not cached; a decode result is cached. This is what lets the
        // `reconcile` retry below actually pick up an avatar that appeared late.
        let Ok(bytes) = std::fs::read(path) else {
            return None;
        };
        let handle = self.gpu.register_image_scaled(&bytes, scene::ICON_MAX_PX);
        self.icon_cache.insert(path.to_string(), handle);
        handle
    }
}

struct BarWin {
    shell_id: ShellId,
    surface: Option<wgpu::Surface<'static>>,
    config: Option<wgpu::SurfaceConfiguration>,
    bar: BossBar,
    hovered: bool,
    hover_anim: HoverAnim,
    last: Instant,
    gesture: DragClick,
    /// Resolved texture for `bar.icon`; re-resolved when the icon path changes.
    icon_tex: Option<TexHandle>,
    self_set: Pos,
    press_cursor: Option<PhysicalPosition<f64>>,
    press_pos: Option<Pos>,
    has_description: bool,
    last_size: (u32, u32),
    scale_factor: f64,
    scale: f32,
    scroll_dirty: bool,
    scroll_last: Option<Instant>,
    last_move: Instant,
    layer: Layer,
}

impl BarWin {
    fn animating(&self) -> bool {
        self.hovered || !self.hover_anim.is_resting()
    }

    fn wants_expanded(&self) -> bool {
        self.has_description && self.animating()
    }

    fn local_position_active(&self, now: Instant) -> bool {
        self.gesture.dragging()
            || self.scroll_dirty
            || now.saturating_duration_since(self.last_move) < SETTLE
    }
}

struct App {
    db: PathBuf,
    base_scale: f32,
    instance: wgpu::Instance,
    core: Option<GpuCore>,
    wins: HashMap<i64, BarWin>,
    start: Instant,
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn open_url(url: &str) {
    if let Err(e) = std::process::Command::new("xdg-open").arg(url).spawn() {
        eprintln!("bossbar-overlay: failed to open {url}: {e}");
    }
}

fn shell_margin(pos: Pos, pinned: bool) -> (i32, i32, i32, i32) {
    let top = pos.y.max(0.0).round() as i32;
    let left = if pinned {
        pos.x.max(0.0).round() as i32
    } else {
        0
    };
    (top, 0, 0, left)
}

fn shell_anchor(pinned: bool) -> Anchor {
    if pinned {
        Anchor::Top | Anchor::Left
    } else {
        Anchor::Top
    }
}

fn axis_drag_delta(horizontal: AxisScroll, vertical: AxisScroll, scale_factor: f64) -> (f64, f64) {
    let scale_factor = scale_factor.max(1.0);
    let dx = if horizontal.absolute != 0.0 {
        -horizontal.absolute / scale_factor
    } else {
        -(horizontal.discrete as f64) * LINE_POINTS
    };
    let dy = if vertical.absolute != 0.0 {
        -vertical.absolute / scale_factor
    } else {
        -(vertical.discrete as f64) * LINE_POINTS
    };
    (dx, dy)
}

fn axis_has_delta(horizontal: AxisScroll, vertical: AxisScroll) -> bool {
    horizontal.absolute != 0.0
        || vertical.absolute != 0.0
        || horizontal.discrete != 0
        || vertical.discrete != 0
}

fn axis_is_discrete(horizontal: AxisScroll, vertical: AxisScroll) -> bool {
    horizontal.discrete != 0 || vertical.discrete != 0
}

fn axis_stopped(horizontal: AxisScroll, vertical: AxisScroll) -> bool {
    horizontal.stop || vertical.stop
}

impl App {
    fn new(db: PathBuf, base_scale: f32) -> Self {
        Self {
            db,
            base_scale: base_scale.max(1.0),
            instance: wgpu::Instance::default(),
            core: None,
            wins: HashMap::new(),
            start: Instant::now(),
        }
    }

    // Fractional scale is preserved (no integer rounding) so a non-integer
    // `base_scale` like 1.25 grows the bars by exactly that fraction.
    fn scale(&self, scale_factor: f64) -> f32 {
        ((self.base_scale as f64) * scale_factor.max(1.0)).max(1.0) as f32
    }

    fn logical_size(size_px: (u32, u32), scale_factor: f64) -> (u32, u32) {
        let scale_factor = scale_factor.max(1.0);
        (
            ((size_px.0 as f64) / scale_factor).ceil().max(1.0) as u32,
            ((size_px.1 as f64) / scale_factor).ceil().max(1.0) as u32,
        )
    }

    fn configured_surface_px(
        configured: (u32, u32),
        requested_px: (u32, u32),
        scale_factor: f64,
    ) -> (u32, u32) {
        let scale_factor = scale_factor.max(1.0);
        let px = |configured: u32, requested: u32| {
            if configured == 0 {
                requested.max(1)
            } else {
                ((configured as f64) * scale_factor).ceil().max(1.0) as u32
            }
        };
        (
            px(configured.0, requested_px.0),
            px(configured.1, requested_px.1),
        )
    }

    fn centered_x(w_px: u32, scale_factor: f64, output_width: f64) -> f64 {
        let wl = w_px as f64 / scale_factor.max(1.0);
        ((output_width - wl) / 2.0).max(0.0)
    }

    fn output_logical_width(ev: &WindowState<i64>, shell_id: ShellId) -> Option<f64> {
        let (w, _) = ev
            .get_unit_with_id(shell_id)?
            .get_xdgoutput_info()?
            .get_logical_size();
        (w > 0).then_some(w as f64)
    }

    fn auto_pos(
        slot: usize,
        w_px: u32,
        h_px: u32,
        scale_factor: f64,
        output_width: Option<f64>,
    ) -> Pos {
        let hl = h_px as f64 / scale_factor.max(1.0);
        // Auto surfaces are compositor-centered by using only a top anchor. Before
        // the first configure we may not have xdg-output details yet, so the
        // fallback x is temporary and is replaced before a drag/scroll pins the bar.
        let x = output_width
            .map(|output_width| Self::centered_x(w_px, scale_factor, output_width))
            .unwrap_or(0.0);
        Pos::new(x, AUTO_TOP + slot as f64 * (hl + AUTO_GAP))
    }

    fn sync_auto_x(&mut self, ev: &WindowState<i64>, shell_id: ShellId, bar_id: i64) -> bool {
        let Some(auto) = self.wins.get(&bar_id).map(|win| win.bar.pos.is_none()) else {
            return false;
        };
        if !auto {
            return true;
        }
        let Some(output_width) = Self::output_logical_width(ev, shell_id) else {
            return false;
        };
        if let Some(win) = self.wins.get_mut(&bar_id) {
            win.self_set.x = Self::centered_x(win.last_size.0, win.scale_factor, output_width);
        }
        true
    }

    fn layer_settings(
        size_px: (u32, u32),
        scale_factor: f64,
        pos: Pos,
        pinned: bool,
        layer: Layer,
    ) -> NewLayerShellSettings {
        NewLayerShellSettings {
            size: Some(Self::logical_size(size_px, scale_factor)),
            layer,
            anchor: shell_anchor(pinned),
            exclusive_zone: None,
            margin: Some(shell_margin(pos, pinned)),
            keyboard_interactivity: KeyboardInteractivity::None,
            output_option: OutputOption::None,
            events_transparent: false,
            namespace: Some("bossbar-overlay".to_string()),
            ..Default::default()
        }
    }

    fn apply_geometry(
        ev: &WindowState<i64>,
        shell_id: ShellId,
        size_px: (u32, u32),
        scale_factor: f64,
        pos: Pos,
        pinned: bool,
    ) {
        if let Some(unit) = ev.get_unit_with_id(shell_id) {
            unit.set_anchor_with_size(
                shell_anchor(pinned),
                Self::logical_size(size_px, scale_factor),
            );
            unit.set_margin(shell_margin(pos, pinned));
        }
    }

    fn create_win(&mut self, ev: &mut WindowState<i64>, bar: BossBar, slot: usize, layer: Layer) {
        let scale_factor = 1.0;
        let scale = self.scale(scale_factor);
        let title_w = scene::title_extent_px(&bar, scale, now_unix());
        let size = scene::bar_window_px(scale, title_w, !bar.icon.is_empty());
        let pinned = bar.pos.is_some();
        let pos = bar
            .pos
            .map(|p| Pos::new(p.x, p.y))
            .unwrap_or_else(|| Self::auto_pos(slot, size.0, size.1, scale_factor, None));
        let shell_id = ShellId::unique();
        let now = Instant::now();
        let has_description = !bar.description.trim().is_empty();
        // The GPU core is created lazily on the first layer surface, so it may not
        // exist yet here; `icon` returns None in that case and `reconcile`
        // resolves the avatar once the core is up.
        let icon_tex = self.core.as_mut().and_then(|c| c.icon(&bar.icon));
        self.wins.insert(
            bar.id,
            BarWin {
                shell_id,
                surface: None,
                config: None,
                bar: bar.clone(),
                icon_tex,
                hovered: false,
                hover_anim: HoverAnim::default(),
                last: now,
                gesture: DragClick::new(DRAG_THRESHOLD),
                self_set: pos,
                press_cursor: None,
                press_pos: None,
                has_description,
                last_size: size,
                scale_factor,
                scale,
                scroll_dirty: false,
                scroll_last: None,
                last_move: now - SETTLE,
                layer,
            },
        );
        ev.append_return_data(ReturnData::NewLayerShell((
            Self::layer_settings(size, scale_factor, pos, pinned, layer),
            shell_id,
            Some(bar.id),
        )));
    }

    fn active_panel_bar(&self) -> Option<i64> {
        self.wins
            .iter()
            .find_map(|(id, win)| (win.has_description && win.hovered).then_some(*id))
            .or_else(|| {
                self.wins
                    .iter()
                    .find_map(|(id, win)| win.wants_expanded().then_some(*id))
            })
    }

    fn sync_layers(&mut self, ev: &WindowState<i64>) {
        let active = self.active_panel_bar();
        for (id, win) in &mut self.wins {
            let target = if active.is_some() && active != Some(*id) {
                Layer::Top
            } else {
                Layer::Overlay
            };
            if win.layer != target {
                win.layer = target;
                if let Some(unit) = ev.get_unit_with_id(win.shell_id) {
                    unit.set_layer(target);
                }
            }
        }
    }

    fn reconcile(&mut self, ev: &mut WindowState<i64>, bars: Vec<BossBar>) {
        let live: HashSet<i64> = bars.iter().map(|bar| bar.id).collect();
        let removed: Vec<i64> = self
            .wins
            .keys()
            .copied()
            .filter(|id| !live.contains(id))
            .collect();
        for id in removed {
            self.close_win(ev, id);
        }

        let now = Instant::now();
        let mut slot = 0usize;
        for bar in bars {
            let this_slot = if bar.pos.is_none() {
                let s = slot;
                slot += 1;
                s
            } else {
                0
            };

            let output_width = self
                .wins
                .get(&bar.id)
                .and_then(|win| Self::output_logical_width(ev, win.shell_id));
            if let Some(win) = self.wins.get_mut(&bar.id) {
                win.has_description = !bar.description.trim().is_empty();
                // Resolve the avatar when the icon path changed, or when it was not
                // yet resolved (e.g. the GPU core came up after this bar was first
                // created). Disjoint field borrows (`wins` vs `core`) are fine.
                let need_icon =
                    win.bar.icon != bar.icon || (win.icon_tex.is_none() && !bar.icon.is_empty());
                if need_icon {
                    win.icon_tex = self.core.as_mut().and_then(|c| c.icon(&bar.icon));
                }
                let local_position_active = win.local_position_active(now);
                let local_pinned = win.bar.pos.is_some();
                let local_pos = win.self_set;
                let db_pos = bar.pos.map(|p| Pos::new(p.x, p.y));
                win.bar = bar;
                let (pos, pinned) = if local_position_active && local_pinned {
                    win.bar.pos = Some(DVec2::new(local_pos.x, local_pos.y));
                    (local_pos, true)
                } else {
                    let pos = db_pos.unwrap_or_else(|| {
                        Self::auto_pos(
                            this_slot,
                            win.last_size.0,
                            win.last_size.1,
                            win.scale_factor,
                            output_width,
                        )
                    });
                    (pos, db_pos.is_some())
                };
                win.self_set = pos;
                Self::apply_geometry(
                    ev,
                    win.shell_id,
                    win.last_size,
                    win.scale_factor,
                    pos,
                    pinned,
                );
                ev.request_refresh(win.shell_id, RefreshRequest::NextFrame);
            } else {
                let layer = if self.active_panel_bar().is_some() {
                    Layer::Top
                } else {
                    Layer::Overlay
                };
                self.create_win(ev, bar, this_slot, layer);
            }
        }
        self.sync_layers(ev);
    }

    fn close_win(&mut self, ev: &mut WindowState<i64>, bar_id: i64) {
        if let Some(mut win) = self.wins.remove(&bar_id) {
            let shell_id = win.shell_id;
            // The wgpu surface borrows raw handles from the layer-shell unit.
            // Drop it before asking layershellev to tear that unit down.
            drop(win.surface.take());
            ev.request_close(shell_id);
        }
    }

    fn drop_surfaces(&mut self) {
        for win in self.wins.values_mut() {
            drop(win.surface.take());
        }
    }

    fn bar_id_for_shell(&self, ev: &WindowState<i64>, shell_id: ShellId) -> Option<i64> {
        ev.get_unit_with_id(shell_id)
            .and_then(|unit| unit.get_binding().copied())
    }

    fn ensure_surface(
        &mut self,
        ev: &WindowState<i64>,
        shell_id: ShellId,
        width: u32,
        height: u32,
    ) -> bool {
        let Some(bar_id) = self.bar_id_for_shell(ev, shell_id) else {
            return false;
        };
        if self
            .wins
            .get(&bar_id)
            .is_none_or(|win| win.surface.is_some())
        {
            return self.wins.contains_key(&bar_id);
        }

        let Some(unit) = ev.get_unit_with_id(shell_id) else {
            return false;
        };
        // SAFETY: the raw Wayland display and surface handles come from the live
        // layershellev unit. `BarWin` drops its wgpu surface before requesting
        // that unit's close, and compositor-initiated closes remove `BarWin`
        // before layershellev destroys the unit.
        let target = match unsafe { wgpu::SurfaceTargetUnsafe::from_window(unit) } {
            Ok(target) => target,
            Err(e) => {
                eprintln!("bossbar-overlay: layer-shell raw handle failed: {e}");
                return false;
            }
        };
        // SAFETY: see the handle lifetime note above.
        let surface = match unsafe { self.instance.create_surface_unsafe(target) } {
            Ok(surface) => surface,
            Err(e) => {
                eprintln!("bossbar-overlay: create layer-shell surface failed: {e}");
                return false;
            }
        };

        if self.core.is_none() {
            let (adapter, device, queue) = ocwin::request_adapter_device(&self.instance, &surface);
            let caps = surface.get_capabilities(&adapter);
            let format = ocwin::srgb_format(&caps);
            let alpha_mode = ocwin::transparent_alpha_mode(&caps);

            let mut gpu = Gpu::new(device, queue, format);
            let textures = scene::register(&mut gpu);
            self.core = Some(GpuCore {
                gpu,
                textures,
                format,
                alpha_mode,
                icon_cache: HashMap::new(),
            });
        }

        let core = self.core.as_ref().expect("core just initialized");
        let config = ocwin::surface_config(core.format, core.alpha_mode, width, height);
        surface.configure(core.gpu.device(), &config);
        if let Some(win) = self.wins.get_mut(&bar_id) {
            win.surface = Some(surface);
            win.config = Some(config);
            true
        } else {
            false
        }
    }

    fn settle_window_size(&mut self, ev: &mut WindowState<i64>, bar_id: i64) -> bool {
        let now = now_unix();
        let Some(win) = self.wins.get_mut(&bar_id) else {
            return false;
        };
        let size = if win.wants_expanded() {
            match self.core.as_ref() {
                Some(core) => scene::expanded_window_px(&core.gpu, &win.bar, win.scale, now),
                None => {
                    let title_w = scene::title_extent_px(&win.bar, win.scale, now);
                    scene::bar_window_px(win.scale, title_w, !win.bar.icon.is_empty())
                }
            }
        } else {
            let title_w = scene::title_extent_px(&win.bar, win.scale, now);
            scene::bar_window_px(win.scale, title_w, !win.bar.icon.is_empty())
        };

        if win.last_size == size {
            return false;
        }

        win.last_size = size;
        let pinned = win.bar.pos.is_some();
        if !pinned && let Some(output_width) = Self::output_logical_width(ev, win.shell_id) {
            win.self_set.x = Self::centered_x(size.0, win.scale_factor, output_width);
        }
        Self::apply_geometry(
            ev,
            win.shell_id,
            size,
            win.scale_factor,
            win.self_set,
            pinned,
        );
        true
    }

    fn configure_surface(&mut self, bar_id: i64, width: u32, height: u32) {
        let Some(core) = self.core.as_ref() else {
            return;
        };
        let Some(win) = self.wins.get_mut(&bar_id) else {
            return;
        };
        let Some(surface) = win.surface.as_ref() else {
            return;
        };
        let needs_config = win
            .config
            .as_ref()
            .is_none_or(|cfg| cfg.width != width || cfg.height != height);
        if needs_config {
            let config = ocwin::surface_config(core.format, core.alpha_mode, width, height);
            surface.configure(core.gpu.device(), &config);
            win.config = Some(config);
        }
    }

    fn render(
        &mut self,
        ev: &mut WindowState<i64>,
        shell_id: ShellId,
        width: u32,
        height: u32,
        scale_factor: f64,
    ) {
        let Some(bar_id) = self.bar_id_for_shell(ev, shell_id) else {
            return;
        };
        let scale = self.scale(scale_factor);
        if let Some(win) = self.wins.get_mut(&bar_id) {
            win.scale_factor = scale_factor.max(1.0);
            win.scale = scale;
        }
        let _ = self.sync_auto_x(ev, shell_id, bar_id);
        if self.settle_window_size(ev, bar_id) {
            return;
        }
        let Some(requested_size) = self.wins.get(&bar_id).map(|win| win.last_size) else {
            return;
        };
        let render_size =
            Self::configured_surface_px((width, height), requested_size, scale_factor);
        if !self.ensure_surface(ev, shell_id, render_size.0, render_size.1) {
            return;
        }
        self.configure_surface(bar_id, render_size.0, render_size.1);

        let now = Instant::now();
        let unix_now = now_unix();
        let breathe = anim::breathe(now.duration_since(self.start), BREATHE_PERIOD);
        let (hover, needs_final_settle) = {
            let Some(win) = self.wins.get_mut(&bar_id) else {
                return;
            };
            let dt = now.duration_since(win.last).min(MAX_STEP);
            win.last = now;
            win.hover_anim
                .approach(if win.hovered { 1.0 } else { 0.0 }, dt, GROW);
            let hover = win.hover_anim.eased();
            // If this frame just eased a leaving hover to rest, the size-settle
            // pass above kept the expanded surface. Queue one more pass to shrink it.
            let title_w = scene::title_extent_px(&win.bar, win.scale, unix_now);
            let collapsed_size = scene::bar_window_px(win.scale, title_w, !win.bar.icon.is_empty());
            (
                hover,
                !win.wants_expanded() && win.last_size != collapsed_size,
            )
        };

        let Some(core) = self.core.as_ref() else {
            return;
        };
        let Some(win) = self.wins.get_mut(&bar_id) else {
            return;
        };
        let Some(surface) = win.surface.as_ref() else {
            return;
        };
        let frame = match surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                if let Some(config) = win.config.as_ref() {
                    surface.configure(core.gpu.device(), config);
                }
                return;
            }
            Err(e) => {
                eprintln!("bossbar-overlay: layer-shell surface error: {e:?}");
                return;
            }
        };
        let Some(config) = win.config.as_ref() else {
            return;
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let quads = scene::build_one(
            &core.gpu,
            &core.textures,
            win.icon_tex,
            win.scale,
            config.width,
            config.height,
            unix_now,
            &win.bar,
            hover,
            breathe,
        );
        let _ = core.gpu.draw(&view, config.width, config.height, &quads);
        frame.present();

        let (shell_id, next) = {
            let next = if needs_final_settle {
                Some(RefreshRequest::NextFrame)
            } else if win.animating() {
                Some(RefreshRequest::At(Instant::now() + FRAME))
            } else if win.bar.since.is_some() {
                Some(RefreshRequest::At(Instant::now() + TICK))
            } else {
                None
            };
            (win.shell_id, next)
        };
        self.sync_layers(ev);
        if let Some(next) = next {
            ev.request_refresh(shell_id, next);
        }
        if let Some(next) = self.settle_scroll_save(bar_id) {
            ev.request_refresh(shell_id, next);
        }
    }

    fn move_bar(&mut self, ev: &mut WindowState<i64>, bar_id: i64, pos: Pos, persist: bool) {
        let Some(win) = self.wins.get_mut(&bar_id) else {
            return;
        };
        win.self_set = pos;
        win.bar.pos = Some(DVec2::new(pos.x, pos.y));
        win.last_move = Instant::now();
        Self::apply_geometry(ev, win.shell_id, win.last_size, win.scale_factor, pos, true);
        ev.request_refresh(win.shell_id, RefreshRequest::NextFrame);
        if persist {
            self.save_position(bar_id, pos);
            self.clear_scroll_dirty(bar_id);
        }
    }

    fn save_position(&self, bar_id: i64, pos: Pos) {
        if let Err(e) = db::set_position(&self.db, bar_id, DVec2::new(pos.x, pos.y)) {
            eprintln!("bossbar-overlay: save position failed: {e}");
        }
    }

    fn clear_scroll_dirty(&mut self, bar_id: i64) {
        if let Some(win) = self.wins.get_mut(&bar_id) {
            win.scroll_dirty = false;
            win.scroll_last = None;
        }
    }

    fn mark_scroll_dirty(&mut self, ev: &mut WindowState<i64>, bar_id: i64) {
        let now = Instant::now();
        if let Some(win) = self.wins.get_mut(&bar_id) {
            win.scroll_dirty = true;
            win.scroll_last = Some(now);
            ev.request_refresh(win.shell_id, RefreshRequest::At(now + SCROLL_SAVE_AFTER));
        }
    }

    fn save_scroll_now(&mut self, bar_id: i64) {
        let pos = self.wins.get_mut(&bar_id).and_then(|win| {
            win.scroll_dirty.then(|| {
                win.scroll_dirty = false;
                win.scroll_last = None;
                win.self_set
            })
        });
        if let Some(pos) = pos {
            self.save_position(bar_id, pos);
        }
    }

    fn settle_scroll_save(&mut self, bar_id: i64) -> Option<RefreshRequest> {
        let now = Instant::now();
        let mut save = None;
        let mut next = None;
        if let Some(win) = self.wins.get_mut(&bar_id)
            && win.scroll_dirty
        {
            let due = win.scroll_last.unwrap_or(now) + SCROLL_SAVE_AFTER;
            if now >= due {
                win.scroll_dirty = false;
                win.scroll_last = None;
                save = Some(win.self_set);
            } else {
                next = Some(RefreshRequest::At(due));
            }
        }
        if let Some(pos) = save {
            self.save_position(bar_id, pos);
        }
        next
    }

    fn handle_dispatch(
        &mut self,
        msg: &DispatchMessage,
        ev: &mut WindowState<i64>,
        shell_id: ShellId,
    ) -> ReturnData<i64> {
        let Some(bar_id) = self.bar_id_for_shell(ev, shell_id) else {
            return ReturnData::None;
        };
        match msg {
            DispatchMessage::RequestRefresh {
                width,
                height,
                scale_float,
                ..
            } => {
                self.render(ev, shell_id, *width, *height, *scale_float);
                ReturnData::None
            }
            DispatchMessage::PreferredScale { scale_float, .. } => {
                let scale = self.scale(*scale_float);
                if let Some(win) = self.wins.get_mut(&bar_id) {
                    win.scale_factor = (*scale_float).max(1.0);
                    win.scale = scale;
                    let pinned = win.bar.pos.is_some();
                    Self::apply_geometry(
                        ev,
                        win.shell_id,
                        win.last_size,
                        win.scale_factor,
                        win.self_set,
                        pinned,
                    );
                    ev.request_refresh(win.shell_id, RefreshRequest::NextFrame);
                }
                ReturnData::None
            }
            DispatchMessage::MouseEnter {
                pointer,
                surface_x,
                surface_y,
                ..
            } => {
                if let Some(win) = self.wins.get_mut(&bar_id) {
                    win.hovered = true;
                    let _ = win
                        .gesture
                        .cursor_moved(PhysicalPosition::new(*surface_x, *surface_y));
                    ev.request_refresh(win.shell_id, RefreshRequest::NextFrame);
                }
                self.sync_layers(ev);
                ReturnData::RequestSetCursorShape(("grab".to_string(), pointer.clone()))
            }
            DispatchMessage::MouseLeave => {
                if let Some(win) = self.wins.get_mut(&bar_id) {
                    win.hovered = false;
                    ev.request_refresh(win.shell_id, RefreshRequest::NextFrame);
                }
                self.sync_layers(ev);
                ReturnData::None
            }
            DispatchMessage::MouseMotion {
                surface_x,
                surface_y,
                ..
            } => {
                let pos = PhysicalPosition::new(*surface_x, *surface_y);
                let mut drag_to = None;
                let mut refresh = None;
                if let Some(win) = self.wins.get_mut(&bar_id) {
                    let started_drag = win.gesture.cursor_moved(pos);
                    if win.gesture.dragging() {
                        if win.press_pos.is_some() {
                            if let (Some(press), Some(cursor)) =
                                (win.press_cursor, win.gesture.cursor())
                            {
                                // Wayland pointer motion is surface-local. Once the
                                // layer surface moves, the next cursor sample is
                                // relative to the moved surface, so keep the press
                                // anchor fixed and advance from the current margin.
                                drag_to = Some(Pos::new(
                                    (win.self_set.x + cursor.x - press.x).max(0.0),
                                    (win.self_set.y + cursor.y - press.y).max(0.0),
                                ));
                            }
                        }
                    } else if started_drag {
                        refresh = Some(win.shell_id);
                    }
                }
                if let Some(pos) = drag_to {
                    self.move_bar(ev, bar_id, pos, false);
                } else if let Some(shell_id) = refresh {
                    ev.request_refresh(shell_id, RefreshRequest::NextFrame);
                }
                ReturnData::None
            }
            DispatchMessage::MouseButton { state, button, .. } if *button == BTN_LEFT => {
                match state {
                    WEnum::Value(ButtonState::Pressed) => {
                        let can_pin = self.sync_auto_x(ev, shell_id, bar_id);
                        if let Some(win) = self.wins.get_mut(&bar_id) {
                            win.gesture.pressed();
                            win.press_cursor = win.gesture.cursor();
                            win.press_pos = can_pin.then_some(win.self_set);
                        }
                    }
                    WEnum::Value(ButtonState::Released) => {
                        let mut drag_to = None;
                        let mut click_url = None;
                        if let Some(win) = self.wins.get_mut(&bar_id) {
                            let was_dragging = win.gesture.dragging();
                            let clicked = win.gesture.released();
                            if clicked {
                                if !win.bar.url.trim().is_empty() {
                                    click_url = Some(win.bar.url.clone());
                                }
                            } else if was_dragging && win.press_pos.is_some() {
                                drag_to = Some(win.self_set);
                            }
                            win.press_cursor = None;
                            win.press_pos = None;
                        }
                        if let Some(url) = click_url {
                            open_url(&url);
                        }
                        if let Some(pos) = drag_to {
                            self.move_bar(ev, bar_id, pos, true);
                        }
                    }
                    _ => {}
                }
                ReturnData::None
            }
            DispatchMessage::MouseButton { state, button, .. } if *button == BTN_RIGHT => {
                if matches!(state, WEnum::Value(ButtonState::Pressed)) {
                    self.drop_surfaces();
                    ReturnData::RequestExit
                } else {
                    ReturnData::None
                }
            }
            DispatchMessage::Axis {
                horizontal,
                vertical,
                ..
            } => {
                if axis_stopped(*horizontal, *vertical) {
                    self.save_scroll_now(bar_id);
                    return ReturnData::None;
                }
                if !axis_has_delta(*horizontal, *vertical) {
                    return ReturnData::None;
                }
                if !self.sync_auto_x(ev, shell_id, bar_id) {
                    return ReturnData::None;
                }
                if let Some(win) = self.wins.get(&bar_id) {
                    let (dx, dy) = axis_drag_delta(*horizontal, *vertical, win.scale_factor);
                    if dx != 0.0 || dy != 0.0 {
                        let pos = Pos::new(
                            (win.self_set.x + dx).max(0.0),
                            (win.self_set.y + dy).max(0.0),
                        );
                        let persist = axis_is_discrete(*horizontal, *vertical);
                        self.move_bar(ev, bar_id, pos, persist);
                        if !persist {
                            self.mark_scroll_dirty(ev, bar_id);
                        }
                    }
                }
                ReturnData::None
            }
            DispatchMessage::Closed => {
                if self
                    .wins
                    .get(&bar_id)
                    .is_some_and(|win| win.shell_id == shell_id)
                {
                    self.wins.remove(&bar_id);
                }
                ReturnData::None
            }
            _ => ReturnData::None,
        }
    }

    fn handle_event(
        &mut self,
        event: LayerShellEvent<i64, Vec<BossBar>>,
        ev: &mut WindowState<i64>,
        shell_id: Option<ShellId>,
    ) -> ReturnData<i64> {
        match event {
            LayerShellEvent::InitRequest => {
                let bars = db::read_once(&self.db).unwrap_or_default();
                self.reconcile(ev, bars);
                ReturnData::None
            }
            LayerShellEvent::UserEvent(bars) => {
                self.reconcile(ev, bars);
                ReturnData::None
            }
            LayerShellEvent::RequestMessages(msg) => match shell_id {
                Some(shell_id) => self.handle_dispatch(msg, ev, shell_id),
                None => ReturnData::None,
            },
            _ => ReturnData::None,
        }
    }
}

pub fn run(db: PathBuf, base_scale: f32) -> Result<(), Box<dyn std::error::Error>> {
    let ev = WindowState::<i64>::new("bossbar-overlay")
        .with_background()
        .with_use_display_handle(true)
        .build()?;

    let _ = db::read_once(&db);

    // layershellev::WindowState::running_with_proxy consumes calloop's channel.
    let (sender, receiver): (
        channel::Sender<Vec<BossBar>>,
        channel::Channel<Vec<BossBar>>,
    ) = channel::channel();
    let watcher_sender = sender.clone();
    db::spawn_watcher(db.clone(), move |bars| watcher_sender.send(bars).is_ok());

    let mut app = App::new(db, base_scale);
    ev.running_with_proxy(receiver, move |event, ev, shell_id| {
        app.handle_event(event, ev, shell_id)
    })?;
    Ok(())
}
