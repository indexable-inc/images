//! The compositor proper: smithay state, the calloop event loop, and the
//! bridge between Wayland commits and the panes wire protocol.
//!
//! There is no rendering and no output device here. Client pixels are copied
//! out of their buffers on commit, diffed, and shipped as damage tiles; the
//! only pacing signal clients see is the `wl_surface` frame callback, fired
//! when the host acks the frame it presented (see `pump` and `on_host_msg`).

mod handlers;
mod input;
mod transport;

#[cfg(feature = "gpu")]
mod gpu;

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use panes_protocol::{Encoding, ToGuest, ToHost, VERSION_MAJOR, VERSION_MINOR, WindowId};
use smithay::input::{Seat, SeatState};
use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{EventLoop, Interest, Mode as CalloopMode, PostAction, channel};
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Display, DisplayHandle};
use smithay::utils::{SERIAL_COUNTER, Transform};
use smithay::wayland::compositor::{
    CompositorClientState, CompositorState, SurfaceAttributes, TraversalAction,
    with_surface_tree_downward,
};
use smithay::wayland::output::OutputManagerState;
use smithay::wayland::selection::data_device::DataDeviceState;
use smithay::wayland::shell::xdg::decoration::XdgDecorationState;
use smithay::wayland::shell::xdg::{PopupSurface, ToplevelSurface, XdgShellState};
use smithay::wayland::shm::ShmState;
use smithay::wayland::socket::ListeningSocketSource;
use tracing::{info, warn};

use crate::cli::Cli;
use crate::frame::FrameStore;
use transport::{HostEvent, HostLink, ListenSpec};

/// The advertised `wl_output` mode. The output is virtual (windows are exported
/// individually and sized by the host), but clients use the mode as a
/// maximize/bounds hint, so advertise something generous.
const VIRTUAL_SIZE: (i32, i32) = (3840, 2160);

/// With no ready host there are no acks, so a slow ticker fires frame
/// callbacks instead: clients keep running (at a throttled rate) rather than
/// wedging forever on a callback that would never come.
const FALLBACK_TICK: Duration = Duration::from_millis(100);

/// Watchdog: ticks (of `FALLBACK_TICK`) an in-flight frame may go unacked
/// before pacing is force-released (~1s). Pacing is one-frame-in-flight, so a
/// host that drops a single ack (e.g. the window is removed host-side between
/// `set_frame` and its next display tick) would otherwise wedge that client
/// permanently.
const INFLIGHT_WATCHDOG_TICKS: u32 = 10;

/// Ceiling for the watchdog's exponential backoff (~8s of `FALLBACK_TICK`s).
/// Every watchdog fire resends a FULL frame -- the largest message on the
/// wire -- so a fixed 1s period on a congested or slow link is a positive
/// feedback loop: each rescue adds the traffic that made the ack late.
/// Consecutive fires double the threshold up to this cap; a real ack resets
/// it to `INFLIGHT_WATCHDOG_TICKS`.
const INFLIGHT_WATCHDOG_MAX_TICKS: u32 = 80;

/// One exported `xdg_toplevel`.
struct Pane {
    id: WindowId,
    toplevel: ToplevelSurface,
    store: FrameStore,
    /// `WindowNew` sent on the *current* host connection.
    announced: bool,
    seq: u64,
    /// Frame seq on the wire, unacked. At most one frame is in flight per
    /// window: that is the whole pacing mechanism.
    inflight: Option<u64>,
    /// Fallback ticks the current in-flight frame has gone unacked; drives
    /// the `INFLIGHT_WATCHDOG_TICKS` rescue in `on_tick`.
    inflight_ticks: u32,
    /// Ticks the watchdog currently waits before firing. Doubles on each
    /// consecutive fire (see `INFLIGHT_WATCHDOG_MAX_TICKS`); reset by a
    /// real ack.
    watchdog_ticks: u32,
    title: String,
    app_id: String,
    min: Option<(u32, u32)>,
    max: Option<(u32, u32)>,
    /// Buffer scale of the last commit (forwarded in `WindowNew`).
    scale: u32,
}

impl Pane {
    fn new(id: WindowId, toplevel: ToplevelSurface) -> Self {
        Self {
            id,
            toplevel,
            store: FrameStore::default(),
            announced: false,
            seq: 0,
            inflight: None,
            inflight_ticks: 0,
            watchdog_ticks: INFLIGHT_WATCHDOG_TICKS,
            title: String::new(),
            app_id: String::new(),
            min: None,
            max: None,
            scale: 1,
        }
    }
}

pub struct App {
    display_handle: DisplayHandle,
    start: Instant,

    compositor_state: CompositorState,
    xdg_shell_state: XdgShellState,
    // Held only to keep their globals alive: neither trait has an accessor.
    _decoration_state: XdgDecorationState,
    _output_manager_state: OutputManagerState,
    shm_state: ShmState,
    /// Clipboard between guest apps. The pixels never leave the guest, but
    /// the global must exist: foot (and other toolkits) hard-fail at startup
    /// when `wl_data_device_manager` is missing.
    data_device_state: DataDeviceState,
    seat_state: SeatState<Self>,
    seat: Seat<Self>,
    output: Output,

    panes: Vec<Pane>,
    /// Popups are tracked only far enough to send their initial configure
    /// (so menu-opening clients do not deadlock); their content is not
    /// exported yet, see the README's protocol-gaps section.
    popups: Vec<PopupSurface>,
    next_window_id: WindowId,

    host: Option<HostLink>,
    /// Window under the host cursor (last `PointerMotion` target).
    pointer_focus: Option<WindowId>,
    /// Window holding `wl_keyboard` focus (last activated / keyed window).
    key_focus: Option<WindowId>,

    #[cfg(feature = "gpu")]
    gpu: Option<gpu::Gpu>,
    #[cfg(feature = "gpu")]
    dmabuf_state: smithay::wayland::dmabuf::DmabufState,
    #[cfg(feature = "gpu")]
    _dmabuf_global: Option<smithay::wayland::dmabuf::DmabufGlobal>,
}

/// Per-client state required by `CompositorHandler::client_compositor_state`.
#[derive(Default)]
pub struct ClientState {
    compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

pub fn run(cli: &Cli) -> anyhow::Result<()> {
    let mut event_loop: EventLoop<'_, App> = EventLoop::try_new().context("create event loop")?;
    let display: Display<App> = Display::new().context("create wayland display")?;
    let mut app = App::new(&display.handle(), cli);

    // Wayland listening socket: clients connect via WAYLAND_DISPLAY.
    let listening_socket =
        ListeningSocketSource::with_name(&cli.socket_name).context("bind wayland socket")?;
    info!(socket = %cli.socket_name, "wayland socket ready (WAYLAND_DISPLAY)");
    event_loop
        .handle()
        .insert_source(listening_socket, |client_stream, (), app| {
            if let Err(err) = app
                .display_handle
                .insert_client(client_stream, Arc::new(ClientState::default()))
            {
                warn!(%err, "failed to insert wayland client");
            }
        })
        .map_err(|err| anyhow::anyhow!("insert wayland listener: {err}"))?;

    // The display itself: readable when clients sent requests.
    event_loop
        .handle()
        .insert_source(
            Generic::new(display, Interest::READ, CalloopMode::Level),
            |_, display, app| {
                // Safety: the display is never dropped while the source is
                // registered (the event loop owns it until process exit).
                unsafe { display.get_mut().dispatch_clients(app) }?;
                Ok(PostAction::Continue)
            },
        )
        .map_err(|err| anyhow::anyhow!("insert display source: {err}"))?;

    // Host transport -> event loop channel.
    let (events_tx, events_rx) = channel::channel::<HostEvent>();
    transport::spawn(&listen_spec(cli), events_tx).context("start host transport")?;
    event_loop
        .handle()
        .insert_source(events_rx, |event, (), app| {
            if let channel::Event::Msg(host_event) = event {
                app.on_host_event(host_event);
            }
        })
        .map_err(|err| anyhow::anyhow!("insert transport channel: {err}"))?;

    // Fallback pacing when no host is connected.
    event_loop
        .handle()
        .insert_source(Timer::from_duration(FALLBACK_TICK), |_, (), app| {
            app.on_tick();
            TimeoutAction::ToDuration(FALLBACK_TICK)
        })
        .map_err(|err| anyhow::anyhow!("insert fallback timer: {err}"))?;

    event_loop
        .run(None, &mut app, |app| {
            // Push out everything the handlers queued this iteration.
            if let Err(err) = app.display_handle.flush_clients() {
                warn!(%err, "flush_clients failed");
            }
        })
        .context("event loop")?;
    Ok(())
}

fn listen_spec(cli: &Cli) -> ListenSpec {
    // clap enforces unix/tcp mutual exclusion; unix wins here only to give
    // the tuple match a total order.
    match (&cli.listen_unix, &cli.listen_tcp) {
        (Some(path), _) => ListenSpec::Unix(path.clone()),
        (None, Some(addr)) => ListenSpec::Tcp(addr.clone()),
        (None, None) => ListenSpec::Vsock(cli.listen_vsock),
    }
}

impl App {
    fn new(dh: &DisplayHandle, cli: &Cli) -> Self {
        let compositor_state = CompositorState::new::<Self>(dh);
        let xdg_shell_state = XdgShellState::new::<Self>(dh);
        let decoration_state = XdgDecorationState::new::<Self>(dh);
        // No extra formats beyond the mandatory ARGB8888/XRGB8888: those are
        // the only ones `copy_shm_buffer` converts.
        let shm_state = ShmState::new::<Self>(dh, Vec::new());
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(dh);
        let data_device_state = DataDeviceState::new::<Self>(dh);
        let mut seat_state = SeatState::new();

        let mut seat: Seat<Self> = seat_state.new_wl_seat(dh, "panes");
        let xkb = smithay::input::keyboard::XkbConfig {
            layout: &cli.xkb_layout,
            ..smithay::input::keyboard::XkbConfig::default()
        };
        // repeat_info(delay=400ms, rate=30/s): the host never forwards OS key
        // repeats, so clients must auto-repeat from this advertisement.
        if let Err(err) = seat.add_keyboard(xkb, 400, 30) {
            warn!(%err, layout = %cli.xkb_layout, "xkb keymap failed; falling back to defaults");
            seat.add_keyboard(smithay::input::keyboard::XkbConfig::default(), 400, 30)
                .expect("default xkb keymap must compile");
        }
        seat.add_pointer();

        // One virtual output. Refresh/scale get overwritten by the host's
        // Hello; until then a bland 60Hz/1x default.
        let output = Output::new(
            "panes".into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "ix".into(),
                model: "panes-virtual".into(),
            },
        );
        output.create_global::<Self>(dh);
        let mode = Mode {
            size: VIRTUAL_SIZE.into(),
            refresh: 60_000,
        };
        output.change_current_state(
            Some(mode),
            Some(Transform::Normal),
            Some(Scale::Integer(1)),
            Some((0, 0).into()),
        );
        output.set_preferred(mode);

        #[cfg(feature = "gpu")]
        let (gpu, dmabuf_state, dmabuf_global) = {
            let mut dmabuf_state = smithay::wayland::dmabuf::DmabufState::new();
            match gpu::Gpu::try_new() {
                Ok(gpu) => {
                    // Advertising linux-dmabuf only when readback actually
                    // works keeps GL clients from binding a global we could
                    // never serve; without it they fall back to shm.
                    let global = dmabuf_state.create_global::<Self>(dh, gpu.formats());
                    info!("GPU readback ready; linux-dmabuf advertised");
                    (Some(gpu), dmabuf_state, Some(global))
                }
                Err(err) => {
                    warn!(%err, "no GPU; running shm-only");
                    (None, dmabuf_state, None)
                }
            }
        };

        Self {
            display_handle: dh.clone(),
            start: Instant::now(),
            compositor_state,
            xdg_shell_state,
            _decoration_state: decoration_state,
            _output_manager_state: output_manager_state,
            shm_state,
            data_device_state,
            seat_state,
            seat,
            output,
            panes: Vec::new(),
            popups: Vec::new(),
            next_window_id: 1,
            host: None,
            pointer_focus: None,
            key_focus: None,
            #[cfg(feature = "gpu")]
            gpu,
            #[cfg(feature = "gpu")]
            dmabuf_state,
            #[cfg(feature = "gpu")]
            _dmabuf_global: dmabuf_global,
        }
    }

    fn now_ms(&self) -> u32 {
        // wl frame callback time wraps at u32 milliseconds by protocol design.
        u32::try_from(self.start.elapsed().as_millis() & u128::from(u32::MAX))
            .expect("masked to 32 bits")
    }

    fn pane_index(&self, id: WindowId) -> Option<usize> {
        self.panes.iter().position(|pane| pane.id == id)
    }

    fn pane_index_of_root(&self, root: &WlSurface) -> Option<usize> {
        self.panes
            .iter()
            .position(|pane| pane.toplevel.wl_surface() == root)
    }

    fn pane_surface(&self, id: WindowId) -> Option<WlSurface> {
        self.pane_index(id)
            .map(|idx| self.panes[idx].toplevel.wl_surface().clone())
    }

    /// Try to move one frame onto the wire for `panes[idx]`, announcing the
    /// window first if this connection has not seen it. When there is
    /// nothing to send, release the window's frame callbacks instead: no
    /// frame means no ack, and without this the client would stall. The only
    /// path that leaves callbacks pending is a frame actually in flight
    /// (that is the throttle; the ack or its watchdog releases them).
    fn pump(&mut self, idx: usize) {
        let now = self.now_ms();
        let Self { host, panes, .. } = self;
        let pane = &mut panes[idx];
        let Some(host) = host.as_ref().filter(|h| h.ready) else {
            // No ready host: the 10Hz fallback tick paces this pane.
            return;
        };
        if !pane.store.has_content() {
            // Content-less commits (initial pre-map commit, commit after
            // unmap) never turn into wire frames, so nothing would ever ack
            // their callbacks; the fallback tick only runs host-less.
            fire_frame_callbacks(pane.toplevel.wl_surface(), now);
            return;
        }
        if pane.inflight.is_some() {
            return;
        }
        if !pane.announced {
            host.send(ToHost::WindowNew {
                id: pane.id,
                title: pane.title.clone(),
                app_id: pane.app_id.clone(),
                width: pane.store.width(),
                height: pane.store.height(),
                scale: pane.scale,
            });
            if pane.min.is_some() || pane.max.is_some() {
                host.send(ToHost::WindowMinMax {
                    id: pane.id,
                    min: pane.min,
                    max: pane.max,
                });
            }
            pane.announced = true;
            tracing::debug!(id = pane.id, "announced WindowNew");
            // This connection has no retained pixels for us yet.
            pane.store.invalidate();
        }
        if let Some(frame) = pane.store.take_frame(host.lz4) {
            pane.seq += 1;
            host.send(ToHost::WindowFrame {
                id: pane.id,
                seq: pane.seq,
                width: frame.width,
                height: frame.height,
                full: frame.full,
                tiles: frame.tiles,
            });
            pane.inflight = Some(pane.seq);
            pane.inflight_ticks = 0;
            tracing::debug!(id = pane.id, seq = pane.seq, "frame sent");
        } else {
            fire_frame_callbacks(pane.toplevel.wl_surface(), now);
        }
    }

    fn on_host_event(&mut self, event: HostEvent) {
        match event {
            HostEvent::Connected(link) => {
                if self.host.is_some() {
                    // One host at a time; the protocol has no multiplexing.
                    warn!(
                        generation = link.generation,
                        "refusing second host connection"
                    );
                    link.close();
                    return;
                }
                link.send(ToHost::Hello {
                    major: VERSION_MAJOR,
                    minor: VERSION_MINOR,
                });
                self.host = Some(link);
            }
            HostEvent::Disconnected { generation } => {
                if self
                    .host
                    .as_ref()
                    .is_some_and(|h| h.generation == generation)
                {
                    self.host = None;
                    for pane in &mut self.panes {
                        pane.inflight = None;
                        pane.inflight_ticks = 0;
                        // A reconnect is a fresh link; backoff earned on the
                        // old one says nothing about it.
                        pane.watchdog_ticks = INFLIGHT_WATCHDOG_TICKS;
                        pane.announced = false;
                        pane.store.invalidate();
                    }
                    info!("host disconnected; windows re-announce on reconnect");
                }
            }
            HostEvent::Message { generation, msg } => {
                if self
                    .host
                    .as_ref()
                    .is_some_and(|h| h.generation == generation)
                {
                    self.on_host_msg(msg);
                }
            }
        }
    }

    fn on_host_msg(&mut self, msg: ToGuest) {
        // Handshake: nothing but Hello counts until the host's Hello passed
        // major-version validation (a peer speaking a different major could
        // otherwise smuggle in misparsed input events).
        if !matches!(msg, ToGuest::Hello { .. }) && !self.host.as_ref().is_some_and(|h| h.ready) {
            warn!("host message before Hello; ignoring");
            return;
        }
        match msg {
            ToGuest::Hello {
                major,
                minor,
                refresh_mhz,
                scale,
                encodings,
            } => self.on_hello(major, minor, refresh_mhz, scale, &encodings),
            ToGuest::Ack { id, seq } => self.on_ack(id, seq),
            ToGuest::Configure {
                id,
                width,
                height,
                scale,
                activated,
            } => self.on_configure(id, width, height, scale, activated),
            ToGuest::CloseRequest { id } => {
                if let Some(idx) = self.pane_index(id) {
                    self.panes[idx].toplevel.send_close();
                }
            }
            ToGuest::Ping { nonce } => {
                if let Some(host) = &self.host {
                    host.send(ToHost::Pong { nonce });
                }
            }
            other => input::handle(self, &other),
        }
    }

    fn on_hello(
        &mut self,
        major: u16,
        minor: u16,
        refresh_mhz: u32,
        scale: u32,
        encodings: &[Encoding],
    ) {
        if major != VERSION_MAJOR {
            warn!(
                host_major = major,
                ours = VERSION_MAJOR,
                "protocol major mismatch; hanging up"
            );
            if let Some(link) = self.host.take() {
                link.close();
            }
            return;
        }
        info!(major, minor, refresh_mhz, scale, "host hello");
        // Advertise the host's real refresh so clients that pace themselves
        // by wl_output pick the right budget (the actual genlock is the
        // ack-driven frame callback, not this number).
        self.output.change_current_state(
            Some(Mode {
                size: VIRTUAL_SIZE.into(),
                refresh: clamp_i32(refresh_mhz.max(1_000)),
            }),
            None,
            Some(Scale::Integer(clamp_i32(scale.max(1)))),
            None,
        );
        if let Some(host) = self.host.as_mut() {
            host.ready = true;
            host.lz4 = encodings.contains(&Encoding::Lz4);
            host.scale = scale.max(1);
        }
        for idx in 0..self.panes.len() {
            self.pump(idx);
        }
    }

    fn on_ack(&mut self, id: WindowId, seq: u64) {
        let now = self.now_ms();
        let Some(idx) = self.pane_index(id) else {
            return;
        };
        // Acks are cumulative: the host coalesces per display tick and acks
        // only the newest presented seq, so any seq >= the awaited one
        // satisfies the wait (an exact-match test would stall forever under
        // coalescing). Older seqs are stale and ignored.
        match self.panes[idx].inflight {
            Some(awaited) if seq >= awaited => {}
            _ => return,
        }
        self.panes[idx].inflight = None;
        self.panes[idx].inflight_ticks = 0;
        // A live ack ends any backoff: the link is moving again.
        self.panes[idx].watchdog_ticks = INFLIGHT_WATCHDOG_TICKS;
        // The host presented: let the client draw the next frame.
        fire_frame_callbacks(self.panes[idx].toplevel.wl_surface(), now);
        // And if commits accumulated while this frame was in flight, send
        // the coalesced delta immediately.
        self.pump(idx);
    }

    // Focus tracking is one-way per window: activated=true steals keyboard
    // focus here, and the previously active window is only deactivated by
    // the host's own paired Configure{activated: false} for it (panes-host
    // sends both on NSWindow key-window changes). A host that omits the
    // deactivate would leave the old window's Activated state set.
    fn on_configure(&mut self, id: WindowId, width: u32, height: u32, scale: u32, activated: bool) {
        use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
        let Some(idx) = self.pane_index(id) else {
            return;
        };
        // The wire carries drawable pixels (panes-protocol convention), but
        // xdg_toplevel.configure takes logical surface coordinates: divide by
        // the window's scale or a Retina client that honors the advertised
        // output scale renders a buffer scale^2 the drawable. div_ceil keeps a
        // stray pixel row covered rather than letterboxed.
        // TODO(index#1686): wl_output's scale is still global (from Hello);
        // honoring a per-window scale that differs from it needs
        // wp_fractional_scale or per-surface preferred scale.
        let scale = scale.max(1);
        let size_valid = width > 0 && height > 0;
        self.panes[idx].toplevel.with_pending_state(|state| {
            if size_valid {
                state.size =
                    Some((clamp_i32(width.div_ceil(scale)), clamp_i32(height.div_ceil(scale))).into());
            }
            if activated {
                state.states.set(xdg_toplevel::State::Activated);
            } else {
                state.states.unset(xdg_toplevel::State::Activated);
            }
        });
        self.panes[idx].toplevel.send_pending_configure();
        if activated {
            let surface = self.panes[idx].toplevel.wl_surface().clone();
            let serial = SERIAL_COUNTER.next_serial();
            if let Some(keyboard) = self.seat.get_keyboard() {
                keyboard.set_focus(self, Some(surface), serial);
            }
            self.key_focus = Some(id);
        } else if self.key_focus == Some(id) {
            let serial = SERIAL_COUNTER.next_serial();
            if let Some(keyboard) = self.seat.get_keyboard() {
                keyboard.set_focus(self, None, serial);
            }
            self.key_focus = None;
        }
    }

    /// 10Hz fallback: with no ready host nothing acks, so release frame
    /// callbacks here. Popups get theirs unconditionally: they are separate
    /// surface trees that never carry wire frames, so no ack ever covers
    /// them.
    ///
    /// With a ready host this doubles as the in-flight watchdog: if an ack
    /// never arrives (a window torn down host-side between `set_frame` and its
    /// next display tick drops the ack on the floor), pacing is force-
    /// released after `INFLIGHT_WATCHDOG_TICKS` and the retained-pixel mirror
    /// invalidated so the next frame ships full (the host's copy can no
    /// longer be trusted as the diff base).
    fn on_tick(&mut self) {
        let now = self.now_ms();
        let host_ready = self.host.as_ref().is_some_and(|h| h.ready);
        if host_ready {
            for idx in 0..self.panes.len() {
                let pane = &mut self.panes[idx];
                if pane.inflight.is_none() {
                    continue;
                }
                pane.inflight_ticks += 1;
                if pane.inflight_ticks < pane.watchdog_ticks {
                    continue;
                }
                // Back off before the next rescue: if this full frame also
                // goes unacked, flooding a struggling link with more fulls
                // only pushes the ack further out.
                pane.watchdog_ticks =
                    (pane.watchdog_ticks * 2).min(INFLIGHT_WATCHDOG_MAX_TICKS);
                warn!(
                    id = pane.id,
                    seq = pane.inflight,
                    next_wait_ticks = pane.watchdog_ticks,
                    "ack never arrived; releasing pacing and resending full"
                );
                pane.inflight = None;
                pane.inflight_ticks = 0;
                pane.store.invalidate();
                fire_frame_callbacks(pane.toplevel.wl_surface(), now);
                self.pump(idx);
            }
        } else {
            for pane in &self.panes {
                fire_frame_callbacks(pane.toplevel.wl_surface(), now);
            }
        }
        for popup in &self.popups {
            fire_frame_callbacks(popup.wl_surface(), now);
        }
    }
}

/// Host-provided u32s land in i32-typed smithay fields. Clamp before the
/// checked conversion: a hostile or buggy peer value must degrade, never
/// panic the compositor (and never silently wrap, hence no `as`).
fn clamp_i32(v: u32) -> i32 {
    const I32_MAX: u32 = 2_147_483_647;
    i32::try_from(v.min(I32_MAX)).expect("clamped to i32::MAX")
}

/// Drain and complete the frame callbacks of `surface` and its subsurfaces.
/// This is the only "you may draw again" signal Wayland clients get, so every
/// path that swallows a commit must eventually route here.
fn fire_frame_callbacks(surface: &WlSurface, time_ms: u32) {
    with_surface_tree_downward(
        surface,
        (),
        |_, _, &()| TraversalAction::DoChildren(()),
        |_, states, &()| {
            for callback in states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .frame_callbacks
                .drain(..)
            {
                callback.done(time_ms);
            }
        },
        |_, _, &()| true,
    );
}
