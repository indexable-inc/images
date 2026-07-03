//! Built-in mock guest for local validation without a VM: serves the panes
//! protocol on a unix socket, maps one toplevel with an animated test pattern
//! (moving gradient + frame counter), and logs every input event it receives.
//!
//! Pointer lock: a right-click press toggles `ToHost::PointerLock` for the
//! window (gated on the host's Hello minor), exercising the host's cursor
//! capture end to end; the `PointerRelative` deltas it produces land in the
//! same input log as everything else.
//!
//! Pacing mirrors the real compositor: exactly one frame in flight, the next
//! render starts when the host acks the previous seq. On a `ProMotion` panel
//! the ack loop should settle at ~120 acks/s; the rate is logged every
//! second.
//!
//! Cross-platform on purpose (plain std + lz4): `--mock-serve` runs headless
//! anywhere, which keeps this code in the Linux build/lint graph and lets the
//! future compositor test against it.

use std::io::{BufReader, BufWriter, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use panes_protocol::{
    ButtonState, Encoding, MINOR_POINTER_LOCK, Rect, Tile, ToGuest, ToHost, VERSION_MAJOR,
    VERSION_MINOR, WindowId, WireError, read_msg, write_msg,
};

const WINDOW_ID: WindowId = 1;
/// evdev right button (input-event-codes.h); a right-click toggles the lock.
const BTN_RIGHT: u32 = 0x111;
/// Window size in points; the buffer is this times the host's scale.
const LOGICAL_WIDTH: u32 = 800;
const LOGICAL_HEIGHT: u32 = 600;
/// Damage tile edge. 256 keeps per-tile payloads comfortably cache-sized and
/// exercises the multi-tile path on any realistic window.
const TILE_EDGE: u32 = 256;
/// How long to wait for the host's Hello before giving up on a connection.
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

pub fn serve(path: &Path) -> std::io::Result<()> {
    // A stale socket file from a previous run would fail the bind.
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    eprintln!("mock: listening on {}", path.display());
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                eprintln!("mock: host connected");
                match handle_conn(stream) {
                    Ok(()) => eprintln!("mock: connection closed"),
                    Err(error) => eprintln!("mock: connection ended: {error}"),
                }
            }
            Err(error) => eprintln!("mock: accept failed: {error}"),
        }
    }
    Ok(())
}

/// What the reader thread distills from host messages for the render loop.
enum HostEvent {
    Hello { scale: u32, lz4: bool, minor: u16 },
    Ack(u64),
    Resize { width: u32, height: u32, scale: u32 },
    Close,
    Ping(u64),
    /// Right-click pressed: flip the window's pointer lock.
    LockToggle,
}

fn handle_conn(stream: UnixStream) -> Result<(), WireError> {
    let read_half = stream.try_clone()?;
    let mut writer = BufWriter::new(stream);
    write_msg(&mut writer, &ToHost::Hello { major: VERSION_MAJOR, minor: VERSION_MINOR })?;
    writer.flush()?;

    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || read_host(read_half, &tx));
    let result = drive(&mut writer, &rx);
    // Unblock the reader (it may sit in read_exact) so join cannot hang.
    let _ = writer.get_ref().shutdown(std::net::Shutdown::Both);
    let _ = reader.join();
    result
}

fn read_host(stream: UnixStream, tx: &mpsc::Sender<HostEvent>) {
    let mut reader = BufReader::new(stream);
    loop {
        let event = match read_msg::<ToGuest>(&mut reader) {
            Ok(ToGuest::Hello { major, minor, refresh_mhz, scale, encodings }) => {
                eprintln!(
                    "mock: host hello {major}.{minor}, refresh {refresh_mhz} mHz, \
                     scale {scale}, encodings {encodings:?}"
                );
                if major != VERSION_MAJOR {
                    eprintln!("mock: protocol major mismatch, hanging up");
                    return;
                }
                HostEvent::Hello {
                    scale: scale.max(1),
                    lz4: encodings.contains(&Encoding::Lz4),
                    minor,
                }
            }
            Ok(ToGuest::Ack { seq, .. }) => HostEvent::Ack(seq),
            Ok(ToGuest::PointerButton { button: BTN_RIGHT, state: ButtonState::Pressed, id }) => {
                eprintln!("mock: right click on window {id}: toggling pointer lock");
                HostEvent::LockToggle
            }
            Ok(ToGuest::Configure { id, width, height, scale, activated }) => {
                eprintln!(
                    "mock: configure window {id}: {width}x{height} scale {scale} \
                     activated {activated}"
                );
                HostEvent::Resize { width, height, scale: scale.max(1) }
            }
            Ok(ToGuest::CloseRequest { id }) => {
                eprintln!("mock: close requested for window {id}");
                HostEvent::Close
            }
            Ok(ToGuest::Ping { nonce }) => HostEvent::Ping(nonce),
            Ok(ToGuest::KeyRepeat { delay_ms, interval_ms }) => {
                // Functional evidence the 1.2 negotiation ran: the host only
                // sends this after our Hello advertised minor >= 2.
                eprintln!("mock: key repeat: delay {delay_ms} ms, interval {interval_ms} ms");
                continue;
            }
            // The point of the mock: prove input arrives, with coordinates.
            Ok(input) => {
                eprintln!("mock: input: {input:?}");
                continue;
            }
            Err(error) => {
                eprintln!("mock: read failed: {error}");
                return;
            }
        };
        if tx.send(event).is_err() {
            return;
        }
    }
}

fn drive(
    writer: &mut BufWriter<UnixStream>,
    rx: &mpsc::Receiver<HostEvent>,
) -> Result<(), WireError> {
    // The host advertises capabilities first; nothing to size buffers on
    // until then.
    let (mut scale, lz4, host_minor) = loop {
        match rx.recv_timeout(HELLO_TIMEOUT) {
            Ok(HostEvent::Hello { scale, lz4, minor }) => break (scale, lz4, minor),
            Ok(_) => {}
            Err(_) => {
                eprintln!("mock: no hello from host, giving up");
                return Ok(());
            }
        }
    };

    let mut width = LOGICAL_WIDTH * scale;
    let mut height = LOGICAL_HEIGHT * scale;
    write_msg(
        writer,
        &ToHost::WindowNew {
            id: WINDOW_ID,
            title: "panes mock".to_string(),
            app_id: "dev.ix.panes.mock".to_string(),
            width,
            height,
            scale,
        },
    )?;
    writer.flush()?;

    let pattern = Pattern::new();
    let mut seq: u64 = 0;
    let send_frame = |writer: &mut BufWriter<UnixStream>,
                          seq: &mut u64,
                          width: u32,
                          height: u32,
                          scale: u32,
                          full: bool|
     -> Result<(), WireError> {
        *seq += 1;
        let pixels = pattern.render(*seq, width, height, scale);
        let tiles = make_tiles(&pixels, width, height, lz4);
        write_msg(writer, &ToHost::WindowFrame { id: WINDOW_ID, seq: *seq, width, height, full, tiles })?;
        writer.flush().map_err(WireError::from)
    };

    send_frame(writer, &mut seq, width, height, scale, true)?;

    let mut acked_in_window: u32 = 0;
    let mut window_start = Instant::now();
    let mut locked = false;
    loop {
        let Ok(event) = rx.recv() else {
            return Ok(()); // host disconnected
        };
        match event {
            HostEvent::Ack(acked) => {
                // A stale ack (pre-resize frame) must not double-schedule.
                if acked != seq {
                    continue;
                }
                acked_in_window += 1;
                let elapsed = window_start.elapsed();
                if elapsed >= Duration::from_secs(1) {
                    let fps = f64::from(acked_in_window) / elapsed.as_secs_f64();
                    eprintln!("mock: {fps:.1} acks/s ({width}x{height} px buffer)");
                    acked_in_window = 0;
                    window_start = Instant::now();
                }
                send_frame(writer, &mut seq, width, height, scale, false)?;
            }
            HostEvent::Resize { width: new_width, height: new_height, scale: new_scale } => {
                if (new_width, new_height, new_scale) == (width, height, scale)
                    || new_width == 0
                    || new_height == 0
                {
                    continue; // activation-only configure
                }
                width = new_width;
                height = new_height;
                scale = new_scale;
                // Full frame at the new size right away; the host stretches
                // stale content until this lands.
                send_frame(writer, &mut seq, width, height, scale, true)?;
            }
            HostEvent::Close => {
                write_msg(writer, &ToHost::WindowGone { id: WINDOW_ID })?;
                writer.flush()?;
                return Ok(());
            }
            HostEvent::Ping(nonce) => {
                write_msg(writer, &ToHost::Pong { nonce })?;
                writer.flush()?;
            }
            HostEvent::LockToggle => {
                // Same negotiation rule as the real compositor: PointerLock
                // is a 1.1 message, never sent to a host that did not
                // advertise it.
                if host_minor < MINOR_POINTER_LOCK {
                    eprintln!("mock: host minor {host_minor} lacks pointer lock; ignoring");
                    continue;
                }
                locked = !locked;
                eprintln!("mock: sending PointerLock locked={locked}");
                write_msg(writer, &ToHost::PointerLock { id: WINDOW_ID, locked })?;
                writer.flush()?;
            }
            HostEvent::Hello { .. } => {}
        }
    }
}

/// 5x7 pixel digits, row bitmasks, bit 4 = leftmost column.
const DIGIT_FONT: [[u8; 7]; 10] = [
    [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E], // 0
    [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E], // 1
    [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F], // 2
    [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E], // 3
    [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02], // 4
    [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E], // 5
    [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E], // 6
    [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08], // 7
    [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E], // 8
    [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C], // 9
];

/// Moving diagonal gradient. Each row is a window into a palette tiled twice,
/// so a frame is `height` memcpys plus the counter overlay, cheap enough to
/// chase 120fps at retina sizes.
struct Pattern {
    /// Palette colors as BGRA bytes, tiled twice so any rotation of the
    /// palette is a contiguous slice.
    tiled: Vec<u8>,
}

/// Palette length in pixels; also the spatial period of the gradient.
const PALETTE: usize = 512;
/// Pixels the gradient advances per frame; visibly fast at 120fps.
const SCROLL_PER_FRAME: u64 = 3;

impl Pattern {
    // The gradient math stays in f64 over values that are exact and small
    // (i < 2 * PALETTE, sin * 110 + 130 lands in 20..=240), so the flagged
    // precision/sign/truncation cannot actually occur in a test pattern.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    fn new() -> Self {
        let mut tiled = Vec::with_capacity(PALETTE * 2 * 4);
        for i in 0..PALETTE * 2 {
            let t = (i % PALETTE) as f64 / PALETTE as f64 * std::f64::consts::TAU;
            let channel = |phase: f64| {
                let value = (t + phase).sin().mul_add(110.0, 130.0);
                value as u8
            };
            // BGRA byte order, opaque (premultiplied with alpha = 255 is
            // just the color itself).
            tiled.extend_from_slice(&[channel(0.0), channel(2.1), channel(4.2), 0xFF]);
        }
        Self { tiled }
    }

    // The scroll shift truncating past 2^64 gradient pixels is beyond any
    // run's lifetime, and it is reduced mod PALETTE immediately anyway.
    #[allow(clippy::cast_possible_truncation)]
    fn render(&self, frame: u64, width: u32, height: u32, scale: u32) -> Vec<u8> {
        let width = width as usize;
        let height = height as usize;
        let mut pixels = vec![0u8; width * height * 4];
        let shift = (frame * SCROLL_PER_FRAME) as usize;
        for y in 0..height {
            let offset = (y + shift) % PALETTE;
            let row = &mut pixels[y * width * 4..(y + 1) * width * 4];
            // The tiled palette is 2 * PALETTE long, so as long as a row
            // chunk is at most PALETTE pixels the slice never wraps.
            let mut x = 0;
            while x < width {
                let chunk = (width - x).min(PALETTE);
                row[x * 4..(x + chunk) * 4]
                    .copy_from_slice(&self.tiled[offset * 4..(offset + chunk) * 4]);
                x += chunk;
            }
        }
        draw_counter(&mut pixels, width, frame, scale);
        pixels
    }
}

/// Blocky frame counter at the top-left so motion (and a stall) is obvious.
fn draw_counter(pixels: &mut [u8], width: usize, value: u64, scale: u32) {
    let cell = 3 * scale as usize; // pixel size of one font pixel
    let digits: Vec<usize> = value
        .to_string()
        .bytes()
        .map(|byte| usize::from(byte - b'0'))
        .collect();
    let margin = 4 * cell;
    for (slot, digit) in digits.iter().enumerate() {
        let glyph = DIGIT_FONT[*digit];
        let origin_x = margin + slot * 6 * cell;
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..5 {
                let lit = bits & (0x10 >> col) != 0;
                let color: [u8; 4] = if lit { [0xFF; 4] } else { [0, 0, 0, 0xFF] };
                for dy in 0..cell {
                    for dx in 0..cell {
                        let x = origin_x + col * cell + dx;
                        let y = margin + row * cell + dy;
                        let at = (y * width + x) * 4;
                        if let Some(px) = pixels.get_mut(at..at + 4) {
                            px.copy_from_slice(&color);
                        }
                    }
                }
            }
        }
    }
}

/// Cut the frame into a `TILE_EDGE` grid, repacking each tile's rows tightly
/// (the wire format is per-tile rows, no stride).
fn make_tiles(pixels: &[u8], width: u32, height: u32, lz4: bool) -> Vec<Tile> {
    let mut tiles = Vec::new();
    let mut y = 0;
    while y < height {
        let tile_h = TILE_EDGE.min(height - y);
        let mut x = 0;
        while x < width {
            let tile_w = TILE_EDGE.min(width - x);
            let mut raw = Vec::with_capacity((tile_w * tile_h * 4) as usize);
            for row in y..y + tile_h {
                let start = ((row * width + x) * 4) as usize;
                raw.extend_from_slice(&pixels[start..start + (tile_w * 4) as usize]);
            }
            let rect = Rect { x, y, w: tile_w, h: tile_h };
            let tile = if lz4 {
                Tile { rect, encoding: Encoding::Lz4, payload: lz4_flex::block::compress(&raw) }
            } else {
                Tile { rect, encoding: Encoding::Raw, payload: raw }
            };
            tiles.push(tile);
            x += tile_w;
        }
        y += tile_h;
    }
    tiles
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decode tiles the way the host does and compare with the source frame:
    /// defends the repack/compress path across the wire boundary.
    #[test]
    fn tiles_reassemble_to_the_rendered_frame() {
        let pattern = Pattern::new();
        let (width, height) = (513u32, 300u32); // deliberately not tile-aligned
        let pixels = pattern.render(7, width, height, 1);
        for lz4 in [false, true] {
            let tiles = make_tiles(&pixels, width, height, lz4);
            let covered: u64 =
                tiles.iter().map(|t| u64::from(t.rect.w) * u64::from(t.rect.h)).sum();
            assert_eq!(covered, u64::from(width) * u64::from(height));

            let mut out = vec![0u8; pixels.len()];
            for tile in &tiles {
                let expected = (tile.rect.w * tile.rect.h * 4) as usize;
                let raw = match tile.encoding {
                    Encoding::Raw => tile.payload.clone(),
                    Encoding::Lz4 => {
                        lz4_flex::block::decompress(&tile.payload, expected).expect("valid lz4")
                    }
                };
                assert_eq!(raw.len(), expected);
                for row in 0..tile.rect.h {
                    let src = (row * tile.rect.w * 4) as usize;
                    let dst = (((tile.rect.y + row) * width + tile.rect.x) * 4) as usize;
                    out[dst..dst + (tile.rect.w * 4) as usize]
                        .copy_from_slice(&raw[src..src + (tile.rect.w * 4) as usize]);
                }
            }
            assert_eq!(out, pixels);
        }
    }

    /// Drive a full mock session over a socketpair, acting as the host:
    /// hello exchange, `WindowNew`, ack-paced frames, close handshake.
    #[test]
    fn mock_speaks_the_protocol_end_to_end() {
        let (host_side, guest_side) = UnixStream::pair().expect("socketpair");
        let server = std::thread::spawn(move || handle_conn(guest_side));

        let mut reader = BufReader::new(host_side.try_clone().expect("clone"));
        let mut writer = BufWriter::new(host_side);
        write_msg(
            &mut writer,
            &ToGuest::Hello {
                major: VERSION_MAJOR,
                minor: VERSION_MINOR,
                refresh_mhz: 120_000,
                scale: 1,
                encodings: vec![Encoding::Raw, Encoding::Lz4],
            },
        )
        .expect("send hello");
        writer.flush().expect("flush");

        let hello: ToHost = read_msg(&mut reader).expect("guest hello");
        assert!(matches!(hello, ToHost::Hello { major: VERSION_MAJOR, .. }));

        let new: ToHost = read_msg(&mut reader).expect("window new");
        let ToHost::WindowNew { id, width, height, .. } = new else {
            panic!("expected WindowNew, got {new:?}");
        };
        assert_eq!(id, WINDOW_ID);
        assert_eq!((width, height), (LOGICAL_WIDTH, LOGICAL_HEIGHT));

        let first: ToHost = read_msg(&mut reader).expect("first frame");
        let ToHost::WindowFrame { seq, full: true, .. } = first else {
            panic!("expected full first WindowFrame, got a different message");
        };

        write_msg(&mut writer, &ToGuest::Ack { id, seq }).expect("ack");
        writer.flush().expect("flush");
        let second: ToHost = read_msg(&mut reader).expect("second frame");
        let ToHost::WindowFrame { seq: next_seq, full: false, .. } = second else {
            panic!("expected incremental WindowFrame, got a different message");
        };
        assert_eq!(next_seq, seq + 1);

        // Right-click press toggles the pointer lock on, a second press
        // toggles it off; releases and other buttons must not toggle.
        let press = |writer: &mut BufWriter<UnixStream>, button: u32, state: ButtonState| {
            write_msg(&mut *writer, &ToGuest::PointerButton { id, button, state })
                .expect("send button");
            writer.flush().expect("flush");
        };
        press(&mut writer, BTN_RIGHT, ButtonState::Pressed);
        let lock: ToHost = read_msg(&mut reader).expect("pointer lock");
        assert!(matches!(lock, ToHost::PointerLock { id: WINDOW_ID, locked: true }));
        press(&mut writer, BTN_RIGHT, ButtonState::Released);
        // Relative deltas while locked are logged, never answered; the next
        // wire message after the release-then-press must be the unlock.
        write_msg(&mut writer, &ToGuest::PointerRelative { id, dx: 3.0, dy: -2.0 })
            .expect("send relative");
        writer.flush().expect("flush");
        press(&mut writer, BTN_RIGHT, ButtonState::Pressed);
        let unlock: ToHost = read_msg(&mut reader).expect("pointer unlock");
        assert!(matches!(unlock, ToHost::PointerLock { id: WINDOW_ID, locked: false }));

        write_msg(&mut writer, &ToGuest::CloseRequest { id }).expect("close request");
        writer.flush().expect("flush");
        // The in-flight frame was not acked, so WindowGone is next.
        let gone: ToHost = read_msg(&mut reader).expect("window gone");
        assert!(matches!(gone, ToHost::WindowGone { id: WINDOW_ID }));

        server.join().expect("server thread").expect("clean close");
    }
}
