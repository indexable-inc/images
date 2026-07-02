//! smithay handler trait impls: the commit path (buffer intake -> `FrameStore`),
//! xdg-shell lifecycle -> wire messages, seat/cursor, and forced server-side
//! decorations.

use panes_protocol::ToHost;
use smithay::input::pointer::CursorImageStatus;
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1;
use smithay::reexports::wayland_server::{Client, Resource as _};
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_seat::WlSeat;
use smithay::reexports::wayland_server::protocol::wl_shm;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Serial, Size};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    BufferAssignment, CompositorClientState, CompositorHandler, CompositorState, SurfaceAttributes,
    get_parent, is_sync_subsurface, with_states,
};
use smithay::wayland::shell::xdg::decoration::XdgDecorationHandler;
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, SurfaceCachedState, ToplevelSurface, XdgShellHandler,
    XdgShellState, XdgToplevelSurfaceData,
};
use smithay::wayland::selection::SelectionHandler;
use smithay::wayland::selection::data_device::{
    ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
    set_data_device_focus,
};
use smithay::wayland::shm::{BufferData, ShmHandler, ShmState, with_buffer_contents};
use smithay::{
    delegate_compositor, delegate_data_device, delegate_output, delegate_seat, delegate_shm,
    delegate_xdg_decoration, delegate_xdg_shell,
};
use tracing::{debug, warn};

use super::{App, ClientState, Pane};
use crate::frame::pack_bgra;

/// One buffer's pixels, converted to the wire's packed BGRA layout.
struct CopiedFrame {
    bgra: Vec<u8>,
    width: u32,
    height: u32,
}

impl CompositorHandler for App {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client
            .get_data::<ClientState>()
            .expect("clients are always inserted with ClientState")
            .compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        // Sync subsurface commits apply when the parent commits; the parent's
        // own commit lands below and re-snapshots everything anyway.
        if is_sync_subsurface(surface) {
            return;
        }

        // Every early return below must release the commit's frame callbacks
        // itself: with a ready host the 10Hz fallback tick does not run, so a
        // callback left pending on a never-pumped surface would wedge its
        // client permanently (seen live: weston clients request a callback on
        // the very first, buffer-less commit).
        let now = self.now_ms();

        // Popups only need their initial configure; their content is not
        // exported (protocol gap, see README).
        if let Some(popup) = self.popups.iter().find(|p| p.wl_surface() == surface) {
            if !popup.is_initial_configure_sent() {
                // The unwrap inside send_configure cannot fail for the
                // initial configure.
                let _ = popup.send_configure();
            }
            super::fire_frame_callbacks(surface, now);
            return;
        }

        let mut root = surface.clone();
        while let Some(parent) = get_parent(&root) {
            root = parent;
        }
        let Some(idx) = self.pane_index_of_root(&root) else {
            // Not a toplevel tree: e.g. a cursor-icon surface. Never pumped,
            // so release its callbacks here.
            super::fire_frame_callbacks(surface, now);
            return;
        };

        // xdg-shell: the first commit (no buffer allowed yet) must be
        // answered with a configure before the client may attach one.
        let initial_configure_sent = with_states(&root, |states| {
            states.data_map.get::<XdgToplevelSurfaceData>().map(|data| {
                data.lock()
                    .expect("no poisoned locks")
                    .initial_configure_sent
            })
        });
        if initial_configure_sent == Some(false) {
            debug!(
                id = self.panes[idx].id,
                "initial commit; sending first configure"
            );
            self.panes[idx].toplevel.send_configure();
            super::fire_frame_callbacks(surface, now);
            return;
        }

        // Only the toplevel's own buffer becomes window content; subsurface
        // pixels are not composited yet (README: known gaps). Buffer intake
        // must run before the min/max sync: it updates `pane.scale`, and a
        // commit that changes buffer_scale would otherwise ship WindowMinMax
        // converted at the stale scale.
        if *surface == root {
            self.intake_buffer(idx);
        }

        self.sync_min_max(idx, &root);

        // Send now if pacing allows, or release the frame callbacks if there
        // is nothing to send (pump handles both).
        self.pump(idx);
    }
}

impl App {
    /// `xdg_toplevel` min/max live in double-buffered surface state, not in a
    /// dedicated event, so they are re-read on every commit and diffed. The
    /// xdg values are logical; the wire wants buffer pixels at the window's
    /// current scale (the host divides by scale for `NSWindow` min/max points).
    // significant_drop_tightening misfires here: the guard is dropped right
    // after its two field reads already, and the suggested
    // `get::<_>().current()` chain borrows a temporary and does not compile.
    #[allow(clippy::significant_drop_tightening)]
    fn sync_min_max(&mut self, idx: usize, root: &WlSurface) {
        let scale = self.panes[idx].scale.max(1);
        let mm = with_states(root, |states| {
            let mut guard = states.cached_state.get::<SurfaceCachedState>();
            let current = guard.current();
            SizeBounds {
                min: size_to_opt(current.min_size, scale),
                max: size_to_opt(current.max_size, scale),
            }
        });
        let pane = &mut self.panes[idx];
        if pane.min == mm.min && pane.max == mm.max {
            return;
        }
        pane.min = mm.min;
        pane.max = mm.max;
        if pane.announced
            && let Some(host) = &self.host
        {
            host.send(ToHost::WindowMinMax {
                id: pane.id,
                min: pane.min,
                max: pane.max,
            });
        }
    }

    /// Take the newly attached buffer (if any) out of the surface state,
    /// copy its pixels into the pane's `FrameStore`, and release it back to
    /// the client immediately (we hold a copy, so the client may reuse it;
    /// this is the classic shm-copy compositor contract).
    // significant_drop_tightening misfires here, same as sync_min_max: the
    // guard already spans only the buffer take + scale read.
    #[allow(clippy::significant_drop_tightening)]
    fn intake_buffer(&mut self, idx: usize) {
        let surface = self.panes[idx].toplevel.wl_surface().clone();
        let intake = with_states(&surface, |states| {
            let mut guard = states.cached_state.get::<SurfaceAttributes>();
            let attrs = guard.current();
            BufferIntake {
                assignment: attrs.buffer.take(),
                scale: attrs.buffer_scale,
            }
        });
        match intake.assignment {
            Some(BufferAssignment::NewBuffer(buffer)) => {
                if let Some(frame) = self.copy_buffer(&buffer) {
                    let pane = &mut self.panes[idx];
                    pane.scale = u32::try_from(intake.scale.max(1)).expect("clamped positive");
                    pane.store.commit(frame.bgra, frame.width, frame.height);
                }
                buffer.release();
            }
            Some(BufferAssignment::Removed) => {
                // Unmap: xdg-shell requires a fresh initial-configure cycle
                // to remap, and the host closes the NSWindow on WindowGone,
                // so the pane resets to the never-announced state (a remap
                // reuses the id; the host sees it as a brand-new window).
                let pane = &mut self.panes[idx];
                if pane.announced
                    && let Some(host) = &self.host
                {
                    host.send(ToHost::WindowGone { id: pane.id });
                }
                pane.announced = false;
                pane.inflight = None;
                pane.inflight_ticks = 0;
                pane.store = crate::frame::FrameStore::default();
            }
            None => {}
        }
    }

    fn copy_buffer(&mut self, buffer: &WlBuffer) -> Option<CopiedFrame> {
        match with_buffer_contents(buffer, |ptr, len, data| copy_shm(ptr, len, &data)) {
            Ok(frame) => frame,
            Err(smithay::wayland::shm::BufferAccessError::NotManaged) => {
                // Not an shm buffer: a dmabuf if the GPU path is live.
                self.copy_dmabuf_buffer(buffer)
            }
            Err(err) => {
                warn!(%err, "shm buffer access failed; skipping frame");
                None
            }
        }
    }

    #[cfg(feature = "gpu")]
    fn copy_dmabuf_buffer(&mut self, buffer: &WlBuffer) -> Option<CopiedFrame> {
        let dmabuf = smithay::wayland::dmabuf::get_dmabuf(buffer).ok()?.clone();
        let gpu = self.gpu.as_mut()?;
        match gpu.readback(&dmabuf) {
            Ok(frame) => Some(CopiedFrame {
                bgra: frame.bgra,
                width: frame.width,
                height: frame.height,
            }),
            Err(err) => {
                warn!(%err, "dmabuf readback failed; skipping frame");
                None
            }
        }
    }

    #[cfg(not(feature = "gpu"))]
    #[allow(clippy::unused_self)]
    fn copy_dmabuf_buffer(&mut self, _buffer: &WlBuffer) -> Option<CopiedFrame> {
        // Without the gpu feature no dmabuf global exists, so clients cannot
        // create such buffers; reaching here means an unmanaged foreign
        // buffer type.
        debug!("unsupported non-shm buffer committed; skipping frame");
        None
    }
}

/// `wl_shm` pool bytes -> packed BGRA. Only ARGB8888/XRGB8888 are advertised
/// (`ShmState::new` extra formats = none), matching the wire's premultiplied
/// BGRA: in little-endian memory both formats already store B,G,R,A(/X).
fn copy_shm(ptr: *const u8, len: usize, data: &BufferData) -> Option<CopiedFrame> {
    let force_opaque = match data.format {
        wl_shm::Format::Argb8888 => false,
        wl_shm::Format::Xrgb8888 => true,
        other => {
            warn!(format = ?other, "unsupported shm format; skipping frame");
            return None;
        }
    };
    let width = u32::try_from(data.width).ok()?;
    let height = u32::try_from(data.height).ok()?;
    let stride = usize::try_from(data.stride).ok()?;
    let offset = usize::try_from(data.offset).ok()?;
    let end = offset.checked_add(stride.checked_mul(height as usize)?)?;
    if end > len || stride < width as usize * crate::frame::BYTES_PER_PIXEL {
        warn!("shm buffer geometry exceeds its pool; skipping frame");
        return None;
    }
    // Safety: with_buffer_contents guarantees ptr..ptr+len maps the shm pool
    // for the duration of the closure (SIGBUS on client-side truncation is
    // caught by smithay and surfaced as BufferAccessError::BadMap).
    let pool = unsafe { std::slice::from_raw_parts(ptr, len) };
    let bgra = pack_bgra(&pool[offset..end], stride, width, height, force_opaque);
    Some(CopiedFrame {
        bgra,
        width,
        height,
    })
}

/// Min/max as the wire wants them: `None` = unconstrained (xdg uses 0).
struct SizeBounds {
    min: Option<(u32, u32)>,
    max: Option<(u32, u32)>,
}

/// Logical xdg size -> buffer pixels at `scale` (saturating: a hostile
/// client-supplied size must not overflow, and `u32::MAX` is "unbounded"
/// anyway).
fn size_to_opt(size: Size<i32, smithay::utils::Logical>, scale: u32) -> Option<(u32, u32)> {
    if size.w <= 0 || size.h <= 0 {
        return None;
    }
    let w = u32::try_from(size.w).expect("checked positive");
    let h = u32::try_from(size.h).expect("checked positive");
    Some((w.saturating_mul(scale), h.saturating_mul(scale)))
}

struct BufferIntake {
    assignment: Option<BufferAssignment>,
    scale: i32,
}

impl BufferHandler for App {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

impl ShmHandler for App {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl SeatHandler for App {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, _image: CursorImageStatus) {
        // TODO(index#1686): serialize Named/Surface cursors into
        // `CursorImage` (render the cursor surface like a window tile). v1
        // always reports "no guest image" so the host keeps its native
        // cursor.
        let Some(id) = self.pointer_focus else {
            return;
        };
        if let Some(host) = &self.host {
            host.send(ToHost::Cursor { id, image: None });
        }
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        // Route the clipboard to whichever client holds keyboard focus, the
        // standard data-device contract: without this, apps could bind the
        // manager but never receive selection offers.
        let client = focused.and_then(|surface| self.display_handle.get_client(surface.id()).ok());
        set_data_device_focus(&self.display_handle, seat, client);
    }
}

// Guest-internal clipboard only (see `App::data_device_state`): selections
// move between guest apps through smithay's built-in plumbing; nothing is
// bridged to the macOS pasteboard yet (protocol gap, see README).
impl SelectionHandler for App {
    type SelectionUserData = ();
}

impl DataDeviceHandler for App {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for App {}
impl ServerDndGrabHandler for App {}

impl smithay::wayland::output::OutputHandler for App {}

impl XdgShellHandler for App {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        // Server-side decorations from the very first configure: the host
        // draws real NSWindow chrome, so client-side shadows/titlebars would
        // be doubled (and their pixels would pollute the exported buffer).
        surface.with_pending_state(|state| {
            state.decoration_mode = Some(zxdg_toplevel_decoration_v1::Mode::ServerSide);
            state.bounds = Some(super::VIRTUAL_SIZE.into());
        });
        let id = self.next_window_id;
        self.next_window_id += 1;
        debug!(id, "new toplevel");
        self.panes.push(Pane::new(id, surface));
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        // Tracked only for the initial configure; content is not exported.
        self.popups.push(surface);
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        surface.send_repositioned(token);
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: WlSeat, _serial: Serial) {
        // Popup grabs need popup export first (README: known gaps).
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let Some(idx) = self
            .panes
            .iter()
            .position(|pane| pane.toplevel.wl_surface() == surface.wl_surface())
        else {
            return;
        };
        let pane = self.panes.remove(idx);
        debug!(id = pane.id, "toplevel destroyed");
        if pane.announced
            && let Some(host) = &self.host
        {
            host.send(ToHost::WindowGone { id: pane.id });
        }
        if self.pointer_focus == Some(pane.id) {
            self.pointer_focus = None;
        }
        if self.key_focus == Some(pane.id) {
            self.key_focus = None;
        }
    }

    fn popup_destroyed(&mut self, surface: PopupSurface) {
        self.popups
            .retain(|popup| popup.wl_surface() != surface.wl_surface());
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        let title = with_states(surface.wl_surface(), |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .and_then(|data| data.lock().expect("no poisoned locks").title.clone())
        })
        .unwrap_or_default();
        let Some(idx) = self
            .panes
            .iter()
            .position(|pane| pane.toplevel.wl_surface() == surface.wl_surface())
        else {
            return;
        };
        let pane = &mut self.panes[idx];
        if pane.title == title {
            return;
        }
        pane.title = title;
        if pane.announced
            && let Some(host) = &self.host
        {
            host.send(ToHost::WindowTitle {
                id: pane.id,
                title: pane.title.clone(),
            });
        }
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        let app_id = with_states(surface.wl_surface(), |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .and_then(|data| data.lock().expect("no poisoned locks").app_id.clone())
        })
        .unwrap_or_default();
        if let Some(pane) = self
            .panes
            .iter_mut()
            .find(|pane| pane.toplevel.wl_surface() == surface.wl_surface())
        {
            // The wire only carries app_id inside WindowNew; a post-announce
            // change cannot be forwarded (protocol gap, see README). Apps
            // set it before mapping in practice.
            pane.app_id = app_id;
        }
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        // Sizing is host-side (the WSLg lesson): never resize on our own,
        // but every maximize/minimize request must still be answered with a
        // configure or the client hangs waiting. This leans on smithay's
        // `send_configure` emitting unconditionally even when no pending
        // state changed (`send_pending_configure` is the change-gated one).
        surface.send_configure();
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        // Same unconditional-emit contract as maximize_request above.
        surface.send_configure();
    }
}

impl XdgDecorationHandler for App {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        // Already forced in new_toplevel; restate for clients that bind the
        // decoration object after the first configure round-trip.
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(zxdg_toplevel_decoration_v1::Mode::ServerSide);
        });
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }

    fn request_mode(
        &mut self,
        toplevel: ToplevelSurface,
        _mode: zxdg_toplevel_decoration_v1::Mode,
    ) {
        // The client's preference is overridden: the host's NSWindow always
        // draws the chrome.
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(zxdg_toplevel_decoration_v1::Mode::ServerSide);
        });
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(zxdg_toplevel_decoration_v1::Mode::ServerSide);
        });
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }
}

#[cfg(feature = "gpu")]
impl smithay::wayland::dmabuf::DmabufHandler for App {
    fn dmabuf_state(&mut self) -> &mut smithay::wayland::dmabuf::DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(
        &mut self,
        _global: &smithay::wayland::dmabuf::DmabufGlobal,
        dmabuf: smithay::backend::allocator::dmabuf::Dmabuf,
        notifier: smithay::wayland::dmabuf::ImportNotifier,
    ) {
        // Validate by importing into the GLES renderer now, so the client
        // learns about an unusable buffer at creation instead of at first
        // frame.
        let imported = self.gpu.as_mut().is_some_and(|gpu| gpu.import(&dmabuf));
        if imported {
            let _ = notifier.successful::<Self>();
        } else {
            notifier.failed();
        }
    }
}

delegate_compositor!(App);
delegate_data_device!(App);
delegate_shm!(App);
delegate_seat!(App);
delegate_output!(App);
delegate_xdg_shell!(App);
delegate_xdg_decoration!(App);

#[cfg(feature = "gpu")]
smithay::delegate_dmabuf!(App);
