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
//!   presented (CAMetalDisplayLink), and the compositor fires Wayland frame
//!   callbacks off that ack, genlocking guest rendering to ProMotion instead
//!   of running an open-loop 120Hz timer.
//! - Windows are xdg_toplevels: title/app_id/min-max map onto NSWindow
//!   properties; interactive resize is host-side (WSLg lesson) and lands as
//!   [`ToGuest::Configure`].

use serde::{Deserialize, Serialize};

/// Bump on any incompatible change; peers refuse mismatched majors.
pub const VERSION: u16 = 1;

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
        version: u16,
    },
    /// A new xdg_toplevel mapped; the host creates its NSWindow on first
    /// `WindowFrame`, so an empty window never flashes.
    WindowNew {
        id: WindowId,
        title: String,
        app_id: String,
        width: u32,
        height: u32,
        /// Buffer scale the guest renders at (host backingScaleFactor echoed
        /// back through `ToGuest::Configure`).
        scale: u32,
    },
    WindowTitle {
        id: WindowId,
        title: String,
    },
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
        tiles: Vec<Tile>,
    },
    /// Toplevel unmapped/destroyed; host closes the NSWindow.
    WindowGone {
        id: WindowId,
    },
    /// Guest-side cursor image for `id` (None = hide, host shows its own).
    Cursor {
        id: WindowId,
        image: Option<CursorImage>,
    },
    Pong {
        nonce: u64,
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
        version: u16,
        /// Host display refresh (mHz), e.g. 120000 for ProMotion; the
        /// compositor advertises it on wl_output.
        refresh_mhz: u32,
        /// NSWindow backingScaleFactor; guest renders at this scale.
        scale: u32,
    },
    /// Presented `seq` for window `id`; compositor fires frame callbacks.
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
        /// evdev button code (BTN_LEFT=0x110, ...).
        button: u32,
        state: ButtonState,
    },
    PointerAxis {
        id: WindowId,
        source: AxisSource,
        horizontal: f64,
        vertical: f64,
        /// wl_pointer v8 value120 wheel steps, when source == Wheel.
        v120: Option<(i32, i32)>,
        stop: bool,
    },
    PointerLeave {
        id: WindowId,
    },
    /// evdev keycode (xkb keycode - 8); repeats are NOT forwarded, guests
    /// auto-repeat from wl_keyboard.repeat_info.
    Key {
        id: WindowId,
        keycode: u32,
        state: ButtonState,
    },
    Ping {
        nonce: u64,
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

/// Cap a single message; a full 5K BGRA frame is ~59 MB, anything past this is
/// a protocol bug, not data.
pub const MAX_FRAME: usize = 256 * 1024 * 1024;

/// Write one message: `[u32 LE len][postcard bytes]`.
pub fn write_msg<T: Serialize>(w: &mut impl std::io::Write, msg: &T) -> Result<(), WireError> {
    let bytes = postcard::to_stdvec(msg)?;
    if bytes.len() > MAX_FRAME {
        return Err(WireError::TooLarge(bytes.len()));
    }
    w.write_all(&u32::try_from(bytes.len()).expect("< MAX_FRAME").to_le_bytes())?;
    w.write_all(&bytes)?;
    Ok(())
}

/// Read one message written by [`write_msg`].
pub fn read_msg<T: for<'de> Deserialize<'de>>(r: &mut impl std::io::Read) -> Result<T, WireError> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME {
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
            tiles: vec![Tile {
                rect: Rect { x: 0, y: 0, w: 2, h: 1 },
                encoding: Encoding::Raw,
                payload: vec![1, 2, 3, 4, 5, 6, 7, 8],
            }],
        };
        let mut buf = Vec::new();
        write_msg(&mut buf, &msg).unwrap();
        let back: ToHost = read_msg(&mut buf.as_slice()).unwrap();
        let ToHost::WindowFrame { id: 7, seq: 42, tiles, .. } = back else {
            panic!("wrong variant");
        };
        assert_eq!(tiles[0].payload.len(), 8);
    }
}
