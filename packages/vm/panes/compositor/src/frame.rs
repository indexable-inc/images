//! Damage tracking and tile encoding, free of Wayland types so it compiles
//! and unit-tests on any platform (the compositor half is Linux-only).
//!
//! Per window: every commit lands the client's pixels in [`FrameStore::commit`]
//! as tightly packed BGRA; when pacing allows a send, [`FrameStore::take_frame`]
//! diffs against the copy the host already holds and returns band-aligned
//! [`Tile`]s covering just the rows that changed, updating the mirror.

use panes_protocol::{Encoding, Rect, Tile};
use rayon::prelude::*;

/// Tiles never exceed this many rows: banding bounds a tile's payload (a
/// 3840-wide band is ~3.9 MB raw) so per-tile LZ4 can fan out across cores
/// later, and a one-pixel change never re-sends more than one band.
pub const BAND_ROWS: u32 = 256;

pub const BYTES_PER_PIXEL: usize = 4;

/// One window frame ready for the wire; field names mirror
/// `ToHost::WindowFrame`.
pub struct EncodedFrame {
    pub width: u32,
    pub height: u32,
    /// True when the host must discard retained contents (first frame after
    /// (re)connect, or a buffer resize).
    pub full: bool,
    pub tiles: Vec<Tile>,
}

/// A tightly packed BGRA image (`width * 4`-byte rows, no padding).
#[derive(Clone)]
struct Pixels {
    data: Vec<u8>,
    width: u32,
    height: u32,
}

/// Per-window pixel state: the latest committed frame plus a mirror of what
/// the connected host currently holds (frames are incremental, so the diff
/// base must be exactly what was last *sent*, not the previous commit).
#[derive(Default)]
pub struct FrameStore {
    current: Option<Pixels>,
    sent: Option<Pixels>,
    /// Set by `commit`, cleared by `take_frame`: content may have changed.
    dirty: bool,
}

impl FrameStore {
    /// Ingest the latest committed client buffer (already converted to packed
    /// BGRA). Cheap bookkeeping only; the diff happens at send time so
    /// commits that pile up between host acks coalesce into one frame.
    pub fn commit(&mut self, bgra: Vec<u8>, width: u32, height: u32) {
        debug_assert_eq!(
            bgra.len(),
            width as usize * height as usize * BYTES_PER_PIXEL
        );
        self.current = Some(Pixels {
            data: bgra,
            width,
            height,
        });
        self.dirty = true;
    }

    pub const fn has_content(&self) -> bool {
        self.current.is_some()
    }

    pub fn width(&self) -> u32 {
        self.current.as_ref().map_or(0, |p| p.width)
    }

    pub fn height(&self) -> u32 {
        self.current.as_ref().map_or(0, |p| p.height)
    }

    /// The host lost its retained copy (disconnect/reconnect): the next
    /// frame must be full.
    pub fn invalidate(&mut self) {
        self.sent = None;
        self.dirty = self.current.is_some();
    }

    /// Diff the latest commit against the host's mirror and encode the delta.
    /// Returns `None` when there is nothing new to send (never committed, or
    /// the commit turned out pixel-identical). On `Some`, the mirror is
    /// updated: the caller is committing to putting this frame on the wire.
    pub fn take_frame(&mut self, allow_lz4: bool) -> Option<EncodedFrame> {
        if !self.dirty {
            return None;
        }
        self.dirty = false;
        let current = self.current.as_ref()?;
        let same_size = self
            .sent
            .as_ref()
            .is_some_and(|s| s.width == current.width && s.height == current.height);
        let rects = if same_size {
            let sent = self.sent.as_ref().expect("same_size implies a sent mirror");
            diff_bands(&sent.data, &current.data, current.width, current.height)
        } else {
            full_bands(current.width, current.height)
        };
        if rects.is_empty() {
            return None;
        }
        // Bands encode in parallel: this runs on the compositor's event-loop
        // thread once per wire frame, and LZ4 over a 2x fullscreen frame is
        // multiple milliseconds serial -- at a 120Hz ack budget of 8.3ms per
        // frame that alone halves the achievable rate. Banding exists for
        // exactly this fan-out (see BAND_ROWS); indexed par_iter keeps tile
        // order.
        let tiles: Vec<Tile> = rects
            .par_iter()
            .map(|rect| encode_tile(&current.data, current.width, *rect, allow_lz4))
            .collect();
        // Update the mirror. The full path clones; the incremental path only
        // copies the changed rows (rects are full-width bands).
        if same_size {
            let current = self.current.as_ref().expect("checked above");
            let sent = self.sent.as_mut().expect("same_size implies a sent mirror");
            let row = current.width as usize * BYTES_PER_PIXEL;
            for rect in &rects {
                let start = rect.y as usize * row;
                let end = start + rect.h as usize * row;
                sent.data[start..end].copy_from_slice(&current.data[start..end]);
            }
        } else {
            self.sent = Some(current.clone());
        }
        let current = self.current.as_ref().expect("checked above");
        Some(EncodedFrame {
            width: current.width,
            height: current.height,
            full: !same_size,
            tiles,
        })
    }
}

/// Band-aligned rects for a whole `width` x `height` frame.
fn full_bands(width: u32, height: u32) -> Vec<Rect> {
    let mut rects = Vec::new();
    let mut y = 0;
    while y < height {
        let h = BAND_ROWS.min(height - y);
        rects.push(Rect {
            x: 0,
            y,
            w: width,
            h,
        });
        y += h;
    }
    rects
}

/// Compare two equally sized packed frames band by band; a changed band is
/// tightened to its first..=last changed rows. Unchanged interior rows within
/// a changed span are re-sent (row-granular tracking is not worth the
/// bookkeeping at <=256-row bands). Bands scan in parallel (same budget
/// argument as the tile encode in `take_frame`: a full-motion 2x frame reads
/// both copies end to end); the indexed collect keeps band order.
fn diff_bands(prev: &[u8], next: &[u8], width: u32, height: u32) -> Vec<Rect> {
    let row_len = width as usize * BYTES_PER_PIXEL;
    let row_changed = |row: u32| {
        let start = row as usize * row_len;
        prev[start..start + row_len] != next[start..start + row_len]
    };
    let bands = height.div_ceil(BAND_ROWS);
    (0..bands)
        .into_par_iter()
        .filter_map(|index| {
            let band = index * BAND_ROWS;
            let end = (band + BAND_ROWS).min(height);
            let mut first = None;
            let mut last = 0;
            for row in band..end {
                if row_changed(row) {
                    if first.is_none() {
                        first = Some(row);
                    }
                    last = row;
                }
            }
            first.map(|first| Rect {
                x: 0,
                y: first,
                w: width,
                h: last - first + 1,
            })
        })
        .collect()
}

/// Encode one damage rect from a packed frame. LZ4 is only kept when it saves
/// at least 10%: shipping near-raw-sized data plus a host-side decode pass is
/// a pure loss on incompressible content (photos, noise), and the host always
/// accepts `Raw`.
///
/// Wire contract for `Encoding::Lz4`: the payload is a raw LZ4 *block*
/// (`lz4_flex::compress` / `decompress`); the decoded size is known from the
/// rect dims (`w * h * 4`), so it is deliberately NOT
/// `compress_prepend_size` (a size-prefixed payload would misdecode on the
/// host and vice versa).
pub fn encode_tile(frame: &[u8], frame_width: u32, rect: Rect, allow_lz4: bool) -> Tile {
    let raw = extract_rect(frame, frame_width, rect);
    if allow_lz4 {
        let compressed = lz4_flex::compress(&raw);
        if compressed.len() * 10 < raw.len() * 9 {
            return Tile {
                rect,
                encoding: Encoding::Lz4,
                payload: compressed,
            };
        }
    }
    Tile {
        rect,
        encoding: Encoding::Raw,
        payload: raw,
    }
}

/// Repack `rect` out of a packed `frame_width`-wide frame into contiguous
/// `rect.w * 4`-byte rows (the wire's tile payload layout).
fn extract_rect(frame: &[u8], frame_width: u32, rect: Rect) -> Vec<u8> {
    let frame_row = frame_width as usize * BYTES_PER_PIXEL;
    let x_bytes = rect.x as usize * BYTES_PER_PIXEL;
    let w_bytes = rect.w as usize * BYTES_PER_PIXEL;
    let mut out = Vec::with_capacity(rect.h as usize * w_bytes);
    for row in rect.y..rect.y + rect.h {
        let start = row as usize * frame_row + x_bytes;
        out.extend_from_slice(&frame[start..start + w_bytes]);
    }
    out
}

/// Repack pixel rows from a strided source (e.g. a `wl_shm` pool slice starting
/// at the buffer's offset) into tight `width * 4`-byte rows, optionally
/// forcing alpha opaque (`wl_shm` `Xrgb8888` leaves the X byte undefined and
/// the wire format is premultiplied BGRA).
pub fn pack_bgra(
    src: &[u8],
    stride: usize,
    width: u32,
    height: u32,
    force_opaque: bool,
) -> Vec<u8> {
    let row = width as usize * BYTES_PER_PIXEL;
    let mut out = Vec::with_capacity(row * height as usize);
    for y in 0..height as usize {
        let start = y * stride;
        out.extend_from_slice(&src[start..start + row]);
    }
    if force_opaque {
        for px in out.chunks_exact_mut(BYTES_PER_PIXEL) {
            px[3] = 0xFF;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic pseudo-random bytes (no rand dep): xorshift over the
    /// index, incompressible enough to defeat LZ4.
    fn noise(len: usize) -> Vec<u8> {
        let mut state = 0x9e37_79b9_u32;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                u8::try_from(state & 0xFF).expect("masked to a byte")
            })
            .collect()
    }

    fn solid(len: usize, byte: u8) -> Vec<u8> {
        vec![byte; len]
    }

    fn decode_tile(tile: &Tile) -> Vec<u8> {
        let expected = tile.rect.w as usize * tile.rect.h as usize * BYTES_PER_PIXEL;
        match tile.encoding {
            Encoding::Raw => tile.payload.clone(),
            Encoding::Lz4 => lz4_flex::decompress(&tile.payload, expected).expect("valid lz4"),
        }
    }

    /// Apply a frame's tiles over a reconstruction buffer, the way the host
    /// composites, so tests verify end-to-end pixel fidelity.
    fn apply(dst: &mut Vec<u8>, frame: &EncodedFrame) {
        let row = frame.width as usize * BYTES_PER_PIXEL;
        if frame.full {
            *dst = vec![0; row * frame.height as usize];
        }
        for tile in &frame.tiles {
            let pixels = decode_tile(tile);
            let w_bytes = tile.rect.w as usize * BYTES_PER_PIXEL;
            for (i, src_row) in pixels.chunks_exact(w_bytes).enumerate() {
                let y = tile.rect.y as usize + i;
                let start = y * row + tile.rect.x as usize * BYTES_PER_PIXEL;
                dst[start..start + w_bytes].copy_from_slice(src_row);
            }
        }
    }

    #[test]
    fn first_frame_is_full_and_band_capped() {
        let mut store = FrameStore::default();
        let (w, h) = (64, 600);
        store.commit(solid(w as usize * h as usize * BYTES_PER_PIXEL, 7), w, h);
        let frame = store.take_frame(true).expect("first frame");
        assert!(frame.full);
        assert_eq!(frame.tiles.len(), 3, "600 rows = 256 + 256 + 88");
        assert!(frame.tiles.iter().all(|t| t.rect.h <= BAND_ROWS));
        let covered: u32 = frame.tiles.iter().map(|t| t.rect.h).sum();
        assert_eq!(covered, h);
    }

    #[test]
    fn unchanged_commit_yields_nothing() {
        let mut store = FrameStore::default();
        let px = solid(16 * 16 * BYTES_PER_PIXEL, 3);
        store.commit(px.clone(), 16, 16);
        store.take_frame(true).expect("first frame");
        store.commit(px, 16, 16);
        assert!(store.take_frame(true).is_none(), "identical pixels");
        assert!(store.take_frame(true).is_none(), "not dirty either");
    }

    #[test]
    fn single_row_change_sends_one_tight_band() {
        let mut store = FrameStore::default();
        let (w, h) = (32, 512);
        let mut px = solid(w as usize * h as usize * BYTES_PER_PIXEL, 0);
        store.commit(px.clone(), w, h);
        store.take_frame(true).expect("first frame");
        // Flip one pixel in row 300 (second band).
        px[300 * w as usize * BYTES_PER_PIXEL] = 0xFF;
        store.commit(px, w, h);
        let frame = store.take_frame(true).expect("delta frame");
        assert!(!frame.full);
        assert_eq!(frame.tiles.len(), 1);
        let rect = frame.tiles[0].rect;
        assert_eq!((rect.y, rect.h, rect.x, rect.w), (300, 1, 0, w));
    }

    #[test]
    fn change_spanning_bands_sends_two_tiles() {
        let mut store = FrameStore::default();
        let (w, h) = (16, 512);
        let mut px = solid(w as usize * h as usize * BYTES_PER_PIXEL, 0);
        store.commit(px.clone(), w, h);
        store.take_frame(true).expect("first frame");
        for row in 250..262_usize {
            px[row * w as usize * BYTES_PER_PIXEL + 1] = 0xAA;
        }
        store.commit(px, w, h);
        let frame = store.take_frame(true).expect("delta frame");
        assert_eq!(frame.tiles.len(), 2, "damage crosses the 256-row boundary");
        assert_eq!(frame.tiles[0].rect.y, 250);
        assert_eq!(frame.tiles[0].rect.h, 6);
        assert_eq!(frame.tiles[1].rect.y, 256);
        assert_eq!(frame.tiles[1].rect.h, 6);
    }

    #[test]
    fn resize_forces_full_frame() {
        let mut store = FrameStore::default();
        store.commit(solid(8 * 8 * BYTES_PER_PIXEL, 1), 8, 8);
        assert!(store.take_frame(true).expect("first").full);
        store.commit(solid(8 * 16 * BYTES_PER_PIXEL, 1), 8, 16);
        let frame = store.take_frame(true).expect("resized");
        assert!(frame.full, "size change invalidates host contents");
        assert_eq!((frame.width, frame.height), (8, 16));
    }

    #[test]
    fn invalidate_forces_full_frame() {
        let mut store = FrameStore::default();
        let px = solid(8 * 8 * BYTES_PER_PIXEL, 9);
        store.commit(px.clone(), 8, 8);
        store.take_frame(true).expect("first");
        store.invalidate();
        let frame = store.take_frame(true).expect("after invalidate");
        assert!(frame.full);
        let mut host = Vec::new();
        apply(&mut host, &frame);
        assert_eq!(host, px);
    }

    #[test]
    fn lz4_roundtrips_and_raw_fallback_kicks_in() {
        // Compressible content goes Lz4 and decodes back exactly.
        let flat = solid(128 * BAND_ROWS as usize * BYTES_PER_PIXEL, 0x42);
        let tile = encode_tile(
            &flat,
            128,
            Rect {
                x: 0,
                y: 0,
                w: 128,
                h: BAND_ROWS,
            },
            true,
        );
        assert_eq!(tile.encoding, Encoding::Lz4);
        assert!(tile.payload.len() < flat.len() / 10);
        assert_eq!(decode_tile(&tile), flat);

        // Incompressible content ships Raw (compressing would save <10%).
        let rnd = noise(64 * 64 * BYTES_PER_PIXEL);
        let tile = encode_tile(
            &rnd,
            64,
            Rect {
                x: 0,
                y: 0,
                w: 64,
                h: 64,
            },
            true,
        );
        assert_eq!(tile.encoding, Encoding::Raw);
        assert_eq!(tile.payload, rnd);

        // A host that never advertised Lz4 gets Raw even for flat content.
        let tile = encode_tile(
            &flat,
            128,
            Rect {
                x: 0,
                y: 0,
                w: 128,
                h: BAND_ROWS,
            },
            false,
        );
        assert_eq!(tile.encoding, Encoding::Raw);
    }

    #[test]
    fn incremental_stream_reconstructs_exactly() {
        let (w, h) = (48, 300);
        let len = w as usize * h as usize * BYTES_PER_PIXEL;
        let mut store = FrameStore::default();
        let mut host = Vec::new();

        let mut px = noise(len);
        store.commit(px.clone(), w, h);
        apply(&mut host, &store.take_frame(true).expect("first"));
        assert_eq!(host, px);

        // Scribble over a few disjoint regions across several commits; only
        // the last pre-send state must be reconstructed.
        for step in 0..4_usize {
            let row = 37 + step * 61;
            let start = row * w as usize * BYTES_PER_PIXEL;
            for b in &mut px[start..start + 24] {
                *b = b.wrapping_add(97);
            }
            store.commit(px.clone(), w, h);
        }
        let frame = store.take_frame(true).expect("coalesced delta");
        assert!(!frame.full);
        apply(&mut host, &frame);
        assert_eq!(host, px, "host mirror must match the guest exactly");
    }

    #[test]
    fn pack_bgra_strips_stride_and_fills_alpha() {
        // 2x2 image in a 3-pixel-wide (12-byte) strided pool.
        let stride = 3 * BYTES_PER_PIXEL;
        let mut src = vec![0_u8; stride * 2];
        src[0..4].copy_from_slice(&[1, 2, 3, 4]);
        src[4..8].copy_from_slice(&[5, 6, 7, 8]);
        src[stride..stride + 4].copy_from_slice(&[9, 10, 11, 12]);
        src[stride + 4..stride + 8].copy_from_slice(&[13, 14, 15, 16]);
        let packed = pack_bgra(&src, stride, 2, 2, false);
        assert_eq!(
            packed,
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
        );
        let opaque = pack_bgra(&src, stride, 2, 2, true);
        assert_eq!(opaque[3], 0xFF);
        assert_eq!(opaque[7], 0xFF);
        assert_eq!(&opaque[0..3], &[1, 2, 3], "color bytes untouched");
    }
}
