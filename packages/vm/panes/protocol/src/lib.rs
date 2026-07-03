//! Wire protocol for seamless guest-Linux windows on a macOS host.
//!
//! One duplex byte stream (guest vsock port <-> host unix socket via libkrun's
//! vsock port map) carries length-prefixed [`postcard`] frames: `[u32 LE len]`
//! then `len` bytes of postcard-encoded [`ToHost`] / [`ToGuest`].
//!
//! Design constraints this encodes (see index#1686):
//! - Frames are damage-driven: [`WindowFrame`] carries only damaged tiles, the
//!   host keeps the previous contents. `Lz4` per-tile because raw 1080p120 is
//!   ~1 GB/s, at the edge of the libkrun vsock budget.
//! - Pacing is ack-driven: the host sends [`ToGuest::Ack`] when a frame is
//!   presented (`CAMetalDisplayLink`), and the compositor fires Wayland frame
//!   callbacks off that ack, genlocking guest rendering to `ProMotion` instead
//!   of running an open-loop 120Hz timer.
//! - Windows are `xdg_toplevels`: title/`app_id`/min-max map onto `NSWindow`
//!   properties; interactive resize is host-side (`WSLg` lesson) and lands as
//!   [`ToGuest::Configure`].
//! - Handshake: both sides send their Hello immediately on connect (no
//!   speak-first ordering); each validates the peer major before any other
//!   message and hangs up on mismatch.

pub mod audio;

use serde::{Deserialize, Serialize};

/// Peers refuse a mismatched major and hang up.
///
/// Postcard has no unknown-variant fallback (an unrecognized enum discriminant
/// is a decode error), so ANY additive message/variant change bumps
/// `VERSION_MINOR` and must only be emitted once the peer's Hello advertised a
/// minor that has it. For the same reason new variants are append-only:
/// postcard encodes the variant index, so inserting one mid-enum renumbers
/// everything after it.
pub const VERSION_MAJOR: u16 = 1;
pub const VERSION_MINOR: u16 = 1;

/// Minor that introduced [`ToHost::PointerLock`] / [`ToGuest::PointerRelative`].
pub const MINOR_POINTER_LOCK: u16 = 1;

/// Guest vsock port the compositor listens on.
pub const VSOCK_PORT: u32 = 7100;

pub type WindowId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Pixel encoding of one damage tile payload. Pixels are BGRA8 premultiplied,
/// `w * 4`-byte rows, no padding (tiles are repacked on the guest).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Encoding {
    Raw,
    Lz4,
}

/// One damaged tile of a window surface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tile {
    pub rect: Rect,
    pub encoding: Encoding,
    #[serde(with = "serde_bytes")]
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToHost {
    Hello {
        major: u16,
        minor: u16,
    },
    /// A new `xdg_toplevel` mapped; the host creates its `NSWindow` on first
    /// `WindowFrame`, so an empty window never flashes. `app_id` is
    /// immutable after map (a post-map change needs a `WindowAppId` message,
    /// minor bump). v1 has no popup/subsurface kind: menus and tooltips get
    /// their configure guest-side but are not exported (needs parent id +
    /// offset on a future `PopupNew`, minor bump).
    WindowNew {
        id: WindowId,
        title: String,
        app_id: String,
        width: u32,
        height: u32,
        /// Buffer scale the guest renders at when this window was announced
        /// (the host `backingScaleFactor` echoed back through
        /// `ToGuest::Configure` when the client honors it, 1 for a
        /// scale-blind client). Also the fixed unit for this window's
        /// `WindowMinMax` sizes: a client that changes buffer scale
        /// mid-connection changes only its frame dimensions, there is no
        /// per-window scale update message.
        scale: u32,
    },
    WindowTitle {
        id: WindowId,
        title: String,
    },
    /// Sizes are buffer pixels at the scale this connection's `WindowNew`
    /// carried (the host divides by that scale for `NSWindow`
    /// `contentMin/MaxSize` points), NOT the current buffer scale.
    WindowMinMax {
        id: WindowId,
        min: Option<(u32, u32)>,
        max: Option<(u32, u32)>,
    },
    /// Full or partial content update. `seq` is echoed in `ToGuest::Ack`.
    WindowFrame {
        id: WindowId,
        seq: u64,
        /// Buffer size; differs from the last `Configure` only mid-resize.
        width: u32,
        height: u32,
        /// True when kept host contents are invalid (first frame and every
        /// buffer resize): pixels outside `tiles` are undefined, host clears.
        /// False = incremental damage over the retained buffer. Without this
        /// flag a partial-damage frame after a resize would composite over
        /// stale wrongly-sized pixels.
        full: bool,
        tiles: Vec<Tile>,
    },
    /// Toplevel unmapped/destroyed; host closes the `NSWindow`.
    WindowGone {
        id: WindowId,
    },
    /// Guest-side cursor image for `id`. None = host shows its own cursor;
    /// v1 cannot express "hide the cursor entirely" (video players), that
    /// needs a distinct Hide state (minor bump).
    Cursor {
        id: WindowId,
        image: Option<CursorImage>,
    },
    Pong {
        nonce: u64,
    },
    /// The window's surface acquired (`locked: true`) or released
    /// (`locked: false`) a `zwp_locked_pointer_v1` lock while holding pointer
    /// focus (mouse-look apps: Minecraft, any GLFW "disabled cursor" client).
    /// While locked the host hides its cursor, dissociates it from mouse
    /// movement, and forwards deltas as [`ToGuest::PointerRelative`] instead
    /// of absolute `PointerMotion`. Since minor 1 ([`MINOR_POINTER_LOCK`]);
    /// only sent once the host's Hello advertised it.
    PointerLock {
        id: WindowId,
        locked: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorImage {
    pub width: u32,
    pub height: u32,
    pub hotspot: (u32, u32),
    #[serde(with = "serde_bytes")]
    pub bgra: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ButtonState {
    Pressed,
    Released,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AxisSource {
    Wheel,
    Finger,
    Continuous,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToGuest {
    Hello {
        major: u16,
        minor: u16,
        /// Host display refresh (mHz), e.g. 120000 for `ProMotion`; the
        /// compositor advertises it on `wl_output`.
        refresh_mhz: u32,
        /// `NSWindow` `backingScaleFactor`; guest renders at this scale.
        scale: u32,
        /// Tile encodings the host decodes; the guest must only emit these.
        /// Extending `Encoding` is gated on this negotiation, not on minor
        /// bumps alone: one unadvertised discriminant kills a whole frame.
        encodings: Vec<Encoding>,
    },
    /// Presented up to `seq` for window `id` (cumulative: the host coalesces
    /// and acks only the newest frame it presented per display tick, so a
    /// guest must treat any `seq >= awaited` as satisfying the wait).
    /// Compositor fires frame callbacks off this.
    Ack {
        id: WindowId,
        seq: u64,
    },
    /// Host-side resize/close/scale; compositor sends xdg configure.
    Configure {
        id: WindowId,
        width: u32,
        height: u32,
        scale: u32,
        activated: bool,
    },
    CloseRequest {
        id: WindowId,
    },
    /// Surface-local pointer coords, already scaled by the guest scale.
    PointerMotion {
        id: WindowId,
        x: f64,
        y: f64,
    },
    PointerButton {
        id: WindowId,
        /// evdev button code (`BTN_LEFT`=0x110, ...).
        button: u32,
        state: ButtonState,
    },
    PointerAxis {
        id: WindowId,
        source: AxisSource,
        horizontal: f64,
        vertical: f64,
        /// `wl_pointer` v8 value120 wheel steps, when source == Wheel.
        v120: Option<(i32, i32)>,
        stop: bool,
    },
    PointerLeave {
        id: WindowId,
    },
    /// evdev keycode (xkb keycode - 8); repeats are NOT forwarded, guests
    /// auto-repeat from `wl_keyboard.repeat_info`.
    Key {
        id: WindowId,
        keycode: u32,
        state: ButtonState,
    },
    Ping {
        nonce: u64,
    },
    /// Relative pointer motion while `id` holds a pointer lock (see
    /// [`ToHost::PointerLock`]). Deltas are buffer pixels, the same unit as
    /// `PointerMotion` coordinates, positive right/down; the compositor feeds
    /// them to `zwp_relative_pointer_v1`. Since minor 1
    /// ([`MINOR_POINTER_LOCK`]); only sent once the guest's Hello advertised
    /// it.
    PointerRelative {
        id: WindowId,
        dx: f64,
        dy: f64,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum WireError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("encode/decode: {0}")]
    Codec(#[from] postcard::Error),
    #[error("frame of {0} bytes exceeds the {MAX_FRAME} cap")]
    TooLarge(usize),
}

/// Cap a single message at 64 MB.
///
/// A full 5K (5120x2880) BGRA frame is ~59 MB and LZ4 only shrinks it, so this
/// fits the worst legitimate frame with headroom while bounding what a hostile
/// length prefix can make `read_msg` allocate.
pub const MAX_FRAME: usize = 64 * 1024 * 1024;

/// Write one message: `[u32 LE len][postcard bytes]`.
///
/// # Errors
/// [`WireError::Codec`] if `msg` fails to encode, [`WireError::TooLarge`] if
/// the encoding exceeds [`MAX_FRAME`] (nothing is written to the stream), and
/// [`WireError::Io`] on a write failure.
pub fn write_msg<T: Serialize>(w: &mut impl std::io::Write, msg: &T) -> Result<(), WireError> {
    let bytes = postcard::to_stdvec(msg)?;
    // Redundant with the MAX_FRAME check (which is < u32::MAX), but keeps the
    // function panic-free by construction rather than by argument.
    let len = u32::try_from(bytes.len()).map_err(|_| WireError::TooLarge(bytes.len()))?;
    if bytes.len() > MAX_FRAME {
        return Err(WireError::TooLarge(bytes.len()));
    }
    w.write_all(&len.to_le_bytes())?;
    w.write_all(&bytes)?;
    Ok(())
}

/// Read one message written by [`write_msg`].
///
/// # Errors
/// [`WireError::Io`] on a short or failed read (including clean EOF),
/// [`WireError::TooLarge`] for a length prefix past [`MAX_FRAME`] (nothing is
/// allocated), and [`WireError::Codec`] if the payload fails to decode.
pub fn read_msg<T: for<'de> Deserialize<'de>>(r: &mut impl std::io::Read) -> Result<T, WireError> {
    read_msg_bounded(r, MAX_FRAME)
}

/// [`read_msg`] with a caller-chosen frame cap.
///
/// The audio stream's biggest legitimate message is a few KiB of PCM (see
/// [`audio::MAX_FRAME`]), so its readers bound a hostile length prefix far
/// below the window stream's 64 MB.
///
/// # Errors
/// As [`read_msg`], with [`WireError::TooLarge`] against `cap` instead of
/// [`MAX_FRAME`].
pub fn read_msg_bounded<T: for<'de> Deserialize<'de>>(
    r: &mut impl std::io::Read,
    cap: usize,
) -> Result<T, WireError> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    // u32 -> usize only narrows on 16-bit targets the workspace does not
    // support; mapping that impossibility to TooLarge (the same rejection an
    // over-cap prefix gets) keeps this panic-free without a lossy fallback.
    let len = usize::try_from(u32::from_le_bytes(len)).map_err(|_| WireError::TooLarge(usize::MAX))?;
    if len > cap {
        return Err(WireError::TooLarge(len));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(postcard::from_bytes(&buf)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let msg = ToHost::WindowFrame {
            id: 7,
            seq: 42,
            width: 640,
            height: 480,
            full: true,
            tiles: vec![Tile {
                rect: Rect { x: 0, y: 0, w: 2, h: 1 },
                encoding: Encoding::Raw,
                payload: vec![1, 2, 3, 4, 5, 6, 7, 8],
            }],
        };
        let mut buf = Vec::new();
        write_msg(&mut buf, &msg).unwrap();
        let back: ToHost = read_msg(&mut buf.as_slice()).unwrap();
        let ToHost::WindowFrame { id: 7, seq: 42, full: true, tiles, .. } = back else {
            panic!("wrong variant");
        };
        assert_eq!(tiles[0].payload.len(), 8);
    }

    #[test]
    fn empty_message_roundtrips() {
        let mut buf = Vec::new();
        write_msg(&mut buf, &ToHost::Pong { nonce: 0 }).unwrap();
        let back: ToHost = read_msg(&mut buf.as_slice()).unwrap();
        assert!(matches!(back, ToHost::Pong { nonce: 0 }));
    }

    #[test]
    fn pointer_lock_and_relative_roundtrip() {
        let mut buf = Vec::new();
        write_msg(&mut buf, &ToHost::PointerLock { id: 3, locked: true }).unwrap();
        let back: ToHost = read_msg(&mut buf.as_slice()).unwrap();
        assert!(matches!(back, ToHost::PointerLock { id: 3, locked: true }));

        let mut buf = Vec::new();
        write_msg(&mut buf, &ToGuest::PointerRelative { id: 3, dx: -1.5, dy: 2.25 }).unwrap();
        let back: ToGuest = read_msg(&mut buf.as_slice()).unwrap();
        let ToGuest::PointerRelative { id: 3, dx, dy } = back else {
            panic!("wrong variant");
        };
        assert!((dx - -1.5).abs() < f64::EPSILON && (dy - 2.25).abs() < f64::EPSILON);
    }

    #[test]
    fn read_rejects_oversized_length_prefix() {
        // A hostile 4-GB-ish prefix must fail fast as TooLarge, not allocate.
        let mut buf = Vec::new();
        buf.extend_from_slice(&u32::try_from(MAX_FRAME + 1).expect("fits u32").to_le_bytes());
        let err = read_msg::<ToHost>(&mut buf.as_slice()).unwrap_err();
        assert!(matches!(err, WireError::TooLarge(n) if n == MAX_FRAME + 1));
    }

    #[test]
    fn read_truncated_stream_is_io_error() {
        let mut buf = Vec::new();
        write_msg(&mut buf, &ToHost::Pong { nonce: 1 }).unwrap();
        buf.truncate(buf.len() - 1);
        let err = read_msg::<ToHost>(&mut buf.as_slice()).unwrap_err();
        assert!(matches!(err, WireError::Io(_)));
    }

    #[test]
    fn write_rejects_over_cap_payload() {
        let msg = ToHost::WindowFrame {
            id: 1,
            seq: 1,
            width: 1,
            height: 1,
            full: true,
            tiles: vec![Tile {
                rect: Rect { x: 0, y: 0, w: 1, h: 1 },
                encoding: Encoding::Raw,
                payload: vec![0u8; MAX_FRAME + 1],
            }],
        };
        let mut buf = Vec::new();
        let err = write_msg(&mut buf, &msg).unwrap_err();
        assert!(matches!(err, WireError::TooLarge(_)));
        assert!(buf.is_empty(), "nothing must hit the stream on failure");
    }
}
