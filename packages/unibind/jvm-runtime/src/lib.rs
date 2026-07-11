//! C-ABI plumbing for the unibind `jvm` backend.
//!
//! The generated glue exports one uniform symbol per function --
//! `fn(args: *const u8, len: usize, out: *mut RawBuf)` -- and the generated
//! Java calls it through the FFM `Linker`. Everything crossing the boundary
//! travels in the length-prefixed wire format this crate implements:
//!
//! - `bool`: one byte, `0` or `1`
//! - integers: little-endian at their Rust width; `isize`/`usize` as 8 bytes
//! - floats: little-endian IEEE-754, 4 or 8 bytes
//! - `String` / bytes: `u32` little-endian byte length, then the bytes
//! - `Option<T>`: one tag byte (`0` none, `1` some), then the payload
//! - `Vec<T>`: `u32` element count, then the elements
//! - maps: `u32` entry count, then key/value pairs
//! - records: their fields in declaration order, no framing
//!
//! A reply starts with a status byte: [`STATUS_OK`] followed by the encoded
//! return value, [`STATUS_ERROR`] followed by a `u32` error-variant index
//! and the `Display` text, or [`STATUS_PANIC`] followed by the panic text.
//! [`invoke`] builds that envelope around the generated body, so a panic
//! surfaces as a Java exception instead of unwinding across the C boundary.
//!
//! Replies land in a caller-provided [`RawBuf`]; Java copies the bytes out
//! and returns the allocation through the interface's exported free symbol,
//! which calls [`free`].

use std::panic::AssertUnwindSafe;

/// A Rust-owned byte buffer handed to the JVM by address.
///
/// The generated Java allocates one of these (24 bytes on every supported
/// target), passes its address as the `out` parameter, copies `len` bytes
/// from `ptr` after the call, and hands all three fields back to the
/// interface's free symbol.
#[repr(C)]
#[derive(Debug)]
pub struct RawBuf {
    /// The allocation, from a `Vec<u8>`; dangling (never null) when empty.
    pub ptr: *mut u8,
    /// Initialized length in bytes.
    pub len: usize,
    /// Allocated capacity in bytes, needed to rebuild the `Vec` for `drop`.
    pub cap: usize,
}

impl RawBuf {
    /// Leak `vec` into its raw parts for the hand-off to Java.
    #[must_use]
    pub fn from_vec(vec: Vec<u8>) -> Self {
        let mut vec = std::mem::ManuallyDrop::new(vec);
        Self {
            ptr: vec.as_mut_ptr(),
            len: vec.len(),
            cap: vec.capacity(),
        }
    }
}

/// Reclaim a buffer previously handed out through a [`RawBuf`].
///
/// A null `ptr` is a no-op, so the Java side can free unconditionally.
///
/// # Safety
///
/// `ptr`, `len`, and `cap` must be the untouched fields of a [`RawBuf`]
/// produced by this crate, freed at most once.
pub unsafe fn free(ptr: *mut u8, len: usize, cap: usize) {
    if ptr.is_null() {
        return;
    }
    drop(unsafe { Vec::from_raw_parts(ptr, len, cap) });
}

/// Reply status: the call returned normally; the return value follows.
pub const STATUS_OK: u8 = 0;
/// Reply status: the call returned the function's declared error; a `u32`
/// variant index and the `Display` text follow.
pub const STATUS_ERROR: u8 = 1;
/// Reply status: the call panicked; the panic text follows.
pub const STATUS_PANIC: u8 = 255;

/// A declared error crossing the boundary: which variant, and its
/// `Display` text.
#[derive(Debug)]
pub struct Failure {
    /// Index of the variant in the error enum's declaration order.
    pub variant: u32,
    /// The error's `Display` rendering.
    pub message: String,
}

/// Run one boundary call: decode the arguments from `args`/`len`, envelope
/// the outcome, and park the encoded reply in `out`.
///
/// `body` decodes its arguments from the [`Reader`], calls the user
/// function, and encodes the return value into the [`Writer`] (which
/// already carries [`STATUS_OK`]); returning a [`Failure`] replaces the
/// reply with the error envelope. A panic anywhere inside -- including the
/// codec's own truncation panics -- becomes the panic envelope, so nothing
/// unwinds across the C boundary.
///
/// # Safety
///
/// `args` must point to `len` readable bytes (`len == 0` permits a null
/// `args`), and `out` must point to writable [`RawBuf`] storage.
pub unsafe fn invoke(
    args: *const u8,
    len: usize,
    out: *mut RawBuf,
    body: impl FnOnce(&mut Reader<'_>, &mut Writer) -> Result<(), Failure>,
) {
    let data: &[u8] = if len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(args, len) }
    };
    let reply = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let mut reader = Reader::new(data);
        let mut writer = Writer::new();
        writer.write_u8(STATUS_OK);
        match body(&mut reader, &mut writer) {
            Ok(()) => writer.into_vec(),
            Err(failure) => {
                let mut writer = Writer::new();
                writer.write_u8(STATUS_ERROR);
                writer.write_u32(failure.variant);
                writer.write_str(&failure.message);
                writer.into_vec()
            }
        }
    }));
    let reply = reply.unwrap_or_else(|panic| {
        let mut writer = Writer::new();
        writer.write_u8(STATUS_PANIC);
        writer.write_str(&panic_text(panic.as_ref()));
        writer.into_vec()
    });
    unsafe { out.write(RawBuf::from_vec(reply)) };
}

/// The human-readable half of a panic payload; `panic!` and `expect` carry
/// a `&str` or `String`, anything else becomes a fixed marker.
fn panic_text(panic: &(dyn std::any::Any + Send)) -> String {
    panic.downcast_ref::<&str>().map_or_else(
        || {
            panic
                .downcast_ref::<String>()
                .cloned()
                .unwrap_or_else(|| "non-string panic payload".to_owned())
        },
        |text| (*text).to_owned(),
    )
}

/// Encode values into the wire format.
#[derive(Debug, Default)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    /// An empty buffer.
    #[must_use]
    pub const fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// The encoded bytes.
    #[must_use]
    pub fn into_vec(self) -> Vec<u8> {
        self.buf
    }

    /// One byte, `0` or `1`.
    pub fn write_bool(&mut self, value: bool) {
        self.buf.push(u8::from(value));
    }

    /// One byte; also the encoding of option tags and reply statuses.
    pub fn write_u8(&mut self, value: u8) {
        self.buf.push(value);
    }

    /// One byte, two's complement.
    pub fn write_i8(&mut self, value: i8) {
        self.buf.push(value.cast_unsigned());
    }

    /// Two bytes, little-endian.
    pub fn write_u16(&mut self, value: u16) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    /// Two bytes, little-endian.
    pub fn write_i16(&mut self, value: i16) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    /// Four bytes, little-endian.
    pub fn write_u32(&mut self, value: u32) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    /// Four bytes, little-endian.
    pub fn write_i32(&mut self, value: i32) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    /// Eight bytes, little-endian.
    pub fn write_u64(&mut self, value: u64) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    /// Eight bytes, little-endian.
    pub fn write_i64(&mut self, value: i64) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    /// Eight bytes, little-endian, regardless of the target's pointer width.
    ///
    /// # Panics
    ///
    /// On a (hypothetical) target where `usize` exceeds 64 bits.
    pub fn write_usize(&mut self, value: usize) {
        self.write_u64(u64::try_from(value).expect("usize fits u64"));
    }

    /// Eight bytes, little-endian, regardless of the target's pointer width.
    ///
    /// # Panics
    ///
    /// On a (hypothetical) target where `isize` exceeds 64 bits.
    pub fn write_isize(&mut self, value: isize) {
        self.write_i64(i64::try_from(value).expect("isize fits i64"));
    }

    /// Four bytes, little-endian IEEE-754.
    pub fn write_f32(&mut self, value: f32) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    /// Eight bytes, little-endian IEEE-754.
    pub fn write_f64(&mut self, value: f64) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    /// A `u32` element count, prefixing a collection's elements.
    ///
    /// # Panics
    ///
    /// If the collection has more than `u32::MAX` elements.
    pub fn write_count(&mut self, count: usize) {
        self.write_u32(u32::try_from(count).expect("collection count fits u32"));
    }

    /// A `u32` byte length, then the UTF-8 bytes.
    ///
    /// # Panics
    ///
    /// If the string is longer than `u32::MAX` bytes.
    pub fn write_str(&mut self, value: &str) {
        self.write_bytes(value.as_bytes());
    }

    /// A `u32` byte length, then the bytes.
    ///
    /// # Panics
    ///
    /// If the slice is longer than `u32::MAX` bytes.
    pub fn write_bytes(&mut self, value: &[u8]) {
        self.write_count(value.len());
        self.buf.extend_from_slice(value);
    }
}

/// Decode values from the wire format.
///
/// Every method panics on malformed input -- a truncated payload, an
/// out-of-range `bool`, invalid UTF-8. The bytes come from the generated
/// Java codec, so malformation is a codec bug, and the panic surfaces as
/// [`STATUS_PANIC`] through the [`invoke`] envelope rather than silently
/// misreading memory.
#[derive(Debug)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    /// Read from `data`, starting at the beginning.
    #[must_use]
    pub const fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// Assert the payload was consumed exactly.
    ///
    /// # Panics
    ///
    /// If bytes remain: the two codecs disagree about the signature.
    pub fn finish(&self) {
        assert!(
            self.pos == self.data.len(),
            "unibind jvm wire: {} unread bytes after the last argument",
            self.data.len() - self.pos,
        );
    }

    /// The next `n` bytes, verbatim.
    ///
    /// # Panics
    ///
    /// If fewer than `n` bytes remain.
    fn take(&mut self, n: usize) -> &'a [u8] {
        let end = self.pos.checked_add(n).expect("offset fits usize");
        assert!(
            end <= self.data.len(),
            "unibind jvm wire: truncated payload (need {n} bytes at offset {}, have {})",
            self.pos,
            self.data.len() - self.pos,
        );
        let bytes = &self.data[self.pos..end];
        self.pos = end;
        bytes
    }

    /// The next `N` bytes as an array, for the fixed-width decoders.
    fn array<const N: usize>(&mut self) -> [u8; N] {
        let mut bytes = [0_u8; N];
        bytes.copy_from_slice(self.take(N));
        bytes
    }

    /// One byte, `0` or `1`.
    ///
    /// # Panics
    ///
    /// On truncation or any other byte value.
    pub fn read_bool(&mut self) -> bool {
        match self.read_u8() {
            0 => false,
            1 => true,
            other => panic!("unibind jvm wire: invalid bool byte {other}"),
        }
    }

    /// One byte; also the decoding of option tags.
    ///
    /// # Panics
    ///
    /// On truncation.
    pub fn read_u8(&mut self) -> u8 {
        self.take(1)[0]
    }

    /// One byte, two's complement.
    ///
    /// # Panics
    ///
    /// On truncation.
    pub fn read_i8(&mut self) -> i8 {
        self.read_u8().cast_signed()
    }

    /// Two bytes, little-endian.
    ///
    /// # Panics
    ///
    /// On truncation.
    pub fn read_u16(&mut self) -> u16 {
        u16::from_le_bytes(self.array())
    }

    /// Two bytes, little-endian.
    ///
    /// # Panics
    ///
    /// On truncation.
    pub fn read_i16(&mut self) -> i16 {
        i16::from_le_bytes(self.array())
    }

    /// Four bytes, little-endian.
    ///
    /// # Panics
    ///
    /// On truncation.
    pub fn read_u32(&mut self) -> u32 {
        u32::from_le_bytes(self.array())
    }

    /// Four bytes, little-endian.
    ///
    /// # Panics
    ///
    /// On truncation.
    pub fn read_i32(&mut self) -> i32 {
        i32::from_le_bytes(self.array())
    }

    /// Eight bytes, little-endian.
    ///
    /// # Panics
    ///
    /// On truncation.
    pub fn read_u64(&mut self) -> u64 {
        u64::from_le_bytes(self.array())
    }

    /// Eight bytes, little-endian.
    ///
    /// # Panics
    ///
    /// On truncation.
    pub fn read_i64(&mut self) -> i64 {
        i64::from_le_bytes(self.array())
    }

    /// Eight bytes, little-endian, narrowed to the target's pointer width.
    ///
    /// # Panics
    ///
    /// On truncation, or if the value exceeds the target's `usize`.
    pub fn read_usize(&mut self) -> usize {
        usize::try_from(self.read_u64()).expect("unibind jvm wire: usize value exceeds this target")
    }

    /// Eight bytes, little-endian, narrowed to the target's pointer width.
    ///
    /// # Panics
    ///
    /// On truncation, or if the value exceeds the target's `isize`.
    pub fn read_isize(&mut self) -> isize {
        isize::try_from(self.read_i64()).expect("unibind jvm wire: isize value exceeds this target")
    }

    /// Four bytes, little-endian IEEE-754.
    ///
    /// # Panics
    ///
    /// On truncation.
    pub fn read_f32(&mut self) -> f32 {
        f32::from_le_bytes(self.array())
    }

    /// Eight bytes, little-endian IEEE-754.
    ///
    /// # Panics
    ///
    /// On truncation.
    pub fn read_f64(&mut self) -> f64 {
        f64::from_le_bytes(self.array())
    }

    /// A `u32` element count, prefixing a collection's elements.
    ///
    /// # Panics
    ///
    /// On truncation.
    pub fn read_count(&mut self) -> usize {
        usize::try_from(self.read_u32()).expect("u32 count fits usize")
    }

    /// A `u32` byte length, then the UTF-8 bytes, borrowed from the payload.
    ///
    /// # Panics
    ///
    /// On truncation or invalid UTF-8.
    pub fn read_str(&mut self) -> &'a str {
        std::str::from_utf8(self.read_bytes()).expect("unibind jvm wire: invalid UTF-8")
    }

    /// Like [`Self::read_str`], but owned.
    ///
    /// # Panics
    ///
    /// On truncation or invalid UTF-8.
    pub fn read_string(&mut self) -> String {
        self.read_str().to_owned()
    }

    /// A `u32` byte length, then the bytes, borrowed from the payload.
    ///
    /// # Panics
    ///
    /// On truncation.
    pub fn read_bytes(&mut self) -> &'a [u8] {
        let len = self.read_count();
        self.take(len)
    }

    /// Like [`Self::read_bytes`], but owned.
    ///
    /// # Panics
    ///
    /// On truncation.
    pub fn read_byte_buf(&mut self) -> Vec<u8> {
        self.read_bytes().to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::{Failure, RawBuf, Reader, Writer, STATUS_ERROR, STATUS_OK, STATUS_PANIC};

    #[test]
    fn round_trips_scalars_and_prefixed_payloads() {
        let mut writer = Writer::new();
        writer.write_bool(true);
        writer.write_i8(-5);
        writer.write_u16(700);
        writer.write_i32(-70_000);
        writer.write_u64(u64::MAX);
        writer.write_isize(-9);
        writer.write_usize(9);
        writer.write_f64(0.5);
        writer.write_str("héllo");
        writer.write_bytes(&[1, 2, 3]);
        let encoded = writer.into_vec();

        let mut reader = Reader::new(&encoded);
        assert!(reader.read_bool());
        assert_eq!(reader.read_i8(), -5);
        assert_eq!(reader.read_u16(), 700);
        assert_eq!(reader.read_i32(), -70_000);
        assert_eq!(reader.read_u64(), u64::MAX);
        assert_eq!(reader.read_isize(), -9);
        assert_eq!(reader.read_usize(), 9);
        assert!((reader.read_f64() - 0.5).abs() < f64::EPSILON);
        assert_eq!(reader.read_str(), "héllo");
        assert_eq!(reader.read_bytes(), [1, 2, 3]);
        reader.finish();
    }

    /// Drive `invoke` and reclaim the reply, returning a copy of its bytes.
    fn invoke_for_reply(
        args: &[u8],
        body: impl FnOnce(&mut Reader<'_>, &mut Writer) -> Result<(), Failure>,
    ) -> Vec<u8> {
        let mut out = RawBuf {
            ptr: std::ptr::null_mut(),
            len: 0,
            cap: 0,
        };
        unsafe { super::invoke(args.as_ptr(), args.len(), &raw mut out, body) };
        let reply = unsafe { std::slice::from_raw_parts(out.ptr, out.len) }.to_vec();
        unsafe { super::free(out.ptr, out.len, out.cap) };
        reply
    }

    #[test]
    fn invoke_envelopes_ok() {
        let args = {
            let mut writer = Writer::new();
            writer.write_i64(20);
            writer.write_i64(22);
            writer.into_vec()
        };
        let reply = invoke_for_reply(&args, |reader, writer| {
            let sum = reader.read_i64() + reader.read_i64();
            writer.write_i64(sum);
            Ok(())
        });
        let mut reader = Reader::new(&reply);
        assert_eq!(reader.read_u8(), STATUS_OK);
        assert_eq!(reader.read_i64(), 42);
        reader.finish();
    }

    #[test]
    fn invoke_envelopes_declared_errors() {
        let reply = invoke_for_reply(&[], |_, _| {
            Err(Failure {
                variant: 1,
                message: "store is gone".to_owned(),
            })
        });
        let mut reader = Reader::new(&reply);
        assert_eq!(reader.read_u8(), STATUS_ERROR);
        assert_eq!(reader.read_u32(), 1);
        assert_eq!(reader.read_str(), "store is gone");
        reader.finish();
    }

    #[test]
    fn invoke_envelopes_panics_including_truncation() {
        // One byte where the body expects an i64: the reader's truncation
        // panic must come back as the panic envelope, not unwind.
        let reply = invoke_for_reply(&[7], |reader, _| {
            reader.read_i64();
            Ok(())
        });
        let mut reader = Reader::new(&reply);
        assert_eq!(reader.read_u8(), STATUS_PANIC);
        assert!(reader.read_str().contains("truncated"));
        reader.finish();
    }
}
