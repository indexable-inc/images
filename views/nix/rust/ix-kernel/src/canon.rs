//! Canonical encoding: the deterministic CBOR subset that request values are
//! hashed through.
//!
//! A memo table is only as trustworthy as the equality it uses. Two requests
//! that mean the same thing must hash the same, and two that mean different
//! things must never hash the same, or a cache hit hands back somebody else's
//! answer. That is an injectivity requirement on the encoder, not a style
//! preference, so the subset is chosen to make injectivity provable rather
//! than tested for.
//!
//! # The subset
//!
//! * **Definite lengths only.** No indefinite-length strings, arrays or maps,
//!   so every item's extent is known from its head.
//! * **Minimal-width integers.** A value is encoded in the shortest of the
//!   five widths that holds it, so each integer has exactly one encoding.
//! * **Map keys sorted bytewise by their encoded form**, and duplicate keys
//!   rejected. Sorting is over the encoded key bytes (RFC 8949 §4.2.1), not
//!   over any in-memory ordering, so the order does not depend on Rust's
//!   `Ord` impls.
//! * **No floats.** They are excluded at the type level: [`CanonValue`] has no
//!   float variant, so a float cannot be encoded rather than being encoded and
//!   then rejected. This is stronger than a runtime check and is why no
//!   `FloatRejected` error exists.
//! * **Strings are UTF-8 and assumed NFC.** Rust's `String` gives UTF-8;
//!   normalisation is *not* performed here, because pulling in a Unicode
//!   normalisation table is a dependency decision for the layer that owns
//!   user-facing text. Callers must hand us NFC. A non-NFC string encodes
//!   fine and simply hashes as a different request, which is a cache miss, not
//!   a wrong answer.
//!
//! # Why this is injective
//!
//! 1. Every item starts with a head byte whose major type names the variant,
//!    so items of different kinds cannot share an encoding.
//! 2. Definite lengths make decoding a single left-to-right walk with no
//!    lookahead and no ambiguity about where an item ends.
//! 3. Minimal-width integers, and the fixed one-byte encodings of null and the
//!    booleans, give each scalar exactly one form.
//! 4. Sorted, duplicate-free map entries give each map exactly one form.
//!
//! So `encode` is total and injective on [`CanonValue`]: distinct values
//! produce distinct byte strings. The [`VERSION`] tag rides along in the
//! domain (see [`crate::Domain::mint`]) rather than in the bytes, so a future
//! `canon-v2` mints different domains instead of colliding with v1 rows in
//! anybody's existing lock file.

use core::fmt;

/// Participates in hashing through the domain, not through the encoded bytes.
pub const VERSION: &str = "canon-v1";

/// A value in the canonical subset. Deliberately smaller than CBOR: no
/// floats, no tags, no simple values beyond `null` and the booleans.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CanonValue {
    Null,
    Bool(bool),
    /// Covers the whole CBOR integer range, which is wider than `i64` in both
    /// directions: unsigned reaches `2^64 - 1` and negative reaches `-2^64`.
    Int(i128),
    Bytes(Vec<u8>),
    Str(String),
    Array(Vec<CanonValue>),
    /// Held as pairs rather than a `BTreeMap` because the canonical order is
    /// over *encoded* keys, which no Rust `Ord` impl reproduces. The encoder
    /// sorts, so construction order is free and never observable.
    Map(Vec<(CanonValue, CanonValue)>),
}

impl CanonValue {
    /// Build a map from string keys, the shape almost every request has.
    #[must_use]
    pub fn map<I, K>(entries: I) -> Self
    where
        I: IntoIterator<Item = (K, Self)>,
        K: Into<String>,
    {
        Self::Map(
            entries
                .into_iter()
                .map(|(key, value)| (Self::Str(key.into()), value))
                .collect(),
        )
    }

    /// Build an array.
    #[must_use]
    pub fn array<I: IntoIterator<Item = Self>>(items: I) -> Self {
        Self::Array(items.into_iter().collect())
    }

    /// Build a string.
    #[must_use]
    pub fn str(text: impl Into<String>) -> Self {
        Self::Str(text.into())
    }

    /// Build an integer from anything that fits.
    #[must_use]
    pub fn int(value: impl Into<i128>) -> Self {
        Self::Int(value.into())
    }
}

/// Why a value could not be canonically encoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CanonError {
    /// Outside the CBOR integer range: `-2^64 ..= 2^64 - 1`.
    IntOutOfRange { value: i128 },
    /// Two entries in one map encoded to the same key bytes. Accepting this
    /// would make the map's encoding depend on which duplicate won.
    DuplicateKey { key_hex: String },
    /// The value nests deeper than [`MAX_DEPTH`]. A depth cap keeps encoding
    /// non-recursive in the pathological case and bounds an attacker's ability
    /// to blow the stack with a deeply nested request.
    TooDeep { limit: u32 },
}

impl fmt::Display for CanonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IntOutOfRange { value } => {
                write!(
                    f,
                    "integer {value} is outside the CBOR range -2^64 ..= 2^64-1"
                )
            }
            Self::DuplicateKey { key_hex } => {
                write!(f, "duplicate map key, encoded as {key_hex}")
            }
            Self::TooDeep { limit } => write!(f, "value nests deeper than {limit} levels"),
        }
    }
}

impl core::error::Error for CanonError {}

/// Deepest nesting the encoder will follow.
pub const MAX_DEPTH: u32 = 128;

/// Largest CBOR unsigned integer, `2^64 - 1`.
const MAX_UINT: i128 = u64::MAX as i128;
/// Smallest CBOR negative integer, `-2^64`. The negative line reaches one
/// further than the positive one because major 1 holds `-1 - n`.
const MIN_NEGINT: i128 = -1 - MAX_UINT;

/// Encode a value into the canonical subset.
pub fn encode(value: &CanonValue) -> Result<Vec<u8>, CanonError> {
    let mut out = Vec::new();
    write_value(value, 0, &mut out)?;
    Ok(out)
}

// CBOR major types, shifted into the top three bits of the head byte.
const MAJOR_UINT: u8 = 0 << 5;
const MAJOR_NEGINT: u8 = 1 << 5;
const MAJOR_BYTES: u8 = 2 << 5;
const MAJOR_TEXT: u8 = 3 << 5;
const MAJOR_ARRAY: u8 = 4 << 5;
const MAJOR_MAP: u8 = 5 << 5;
const MAJOR_SIMPLE: u8 = 7 << 5;

const SIMPLE_FALSE: u8 = 20;
const SIMPLE_TRUE: u8 = 21;
const SIMPLE_NULL: u8 = 22;

fn write_value(value: &CanonValue, depth: u32, out: &mut Vec<u8>) -> Result<(), CanonError> {
    if depth > MAX_DEPTH {
        return Err(CanonError::TooDeep { limit: MAX_DEPTH });
    }
    match value {
        CanonValue::Null => out.push(MAJOR_SIMPLE | SIMPLE_NULL),
        CanonValue::Bool(false) => out.push(MAJOR_SIMPLE | SIMPLE_FALSE),
        CanonValue::Bool(true) => out.push(MAJOR_SIMPLE | SIMPLE_TRUE),
        CanonValue::Int(n) => write_int(*n, out)?,
        CanonValue::Bytes(bytes) => {
            write_head(MAJOR_BYTES, bytes.len() as u64, out);
            out.extend_from_slice(bytes);
        }
        CanonValue::Str(text) => {
            write_head(MAJOR_TEXT, text.len() as u64, out);
            out.extend_from_slice(text.as_bytes());
        }
        CanonValue::Array(items) => {
            write_head(MAJOR_ARRAY, items.len() as u64, out);
            for item in items {
                write_value(item, depth + 1, out)?;
            }
        }
        CanonValue::Map(entries) => write_map(entries, depth, out)?,
    }
    Ok(())
}

fn write_map(
    entries: &[(CanonValue, CanonValue)],
    depth: u32,
    out: &mut Vec<u8>,
) -> Result<(), CanonError> {
    // Encode each key first: the canonical order is over encoded key bytes,
    // so it cannot be decided without encoding.
    let mut encoded = Vec::with_capacity(entries.len());
    for (key, value) in entries {
        let mut key_bytes = Vec::new();
        write_value(key, depth + 1, &mut key_bytes)?;
        encoded.push((key_bytes, value));
    }
    encoded.sort_by(|left, right| left.0.cmp(&right.0));
    if let Some(duplicate) = first_duplicate(&encoded) {
        return Err(CanonError::DuplicateKey {
            key_hex: duplicate.iter().map(|b| format!("{b:02x}")).collect(),
        });
    }

    write_head(MAJOR_MAP, encoded.len() as u64, out);
    for (key_bytes, value) in &encoded {
        out.extend_from_slice(key_bytes);
        write_value(value, depth + 1, out)?;
    }
    Ok(())
}

fn first_duplicate<'a>(encoded: &'a [(Vec<u8>, &CanonValue)]) -> Option<&'a [u8]> {
    encoded
        .windows(2)
        .find_map(|pair| match (pair.first(), pair.last()) {
            (Some(left), Some(right)) if left.0 == right.0 => Some(left.0.as_slice()),
            _ => None,
        })
}

fn write_int(value: i128, out: &mut Vec<u8>) -> Result<(), CanonError> {
    // CBOR splits the integer line at zero: non-negative values are major 0
    // holding n, negative values are major 1 holding -1 - n, which is why the
    // negative range reaches one further than the positive one.
    if !(MIN_NEGINT..=MAX_UINT).contains(&value) {
        return Err(CanonError::IntOutOfRange { value });
    }
    // Range-checked above, so neither the negation nor the narrowing can trap.
    let (major, magnitude) = if value >= 0 {
        (MAJOR_UINT, value)
    } else {
        (MAJOR_NEGINT, -1 - value)
    };
    let magnitude = u64::try_from(magnitude).map_err(|_| CanonError::IntOutOfRange { value })?;
    write_head(major, magnitude, out);
    Ok(())
}

/// Write a head byte plus the shortest argument encoding that holds `value`.
fn write_head(major: u8, value: u64, out: &mut Vec<u8>) {
    match value {
        // Values below 24 live in the head byte itself; 24..=27 are the
        // reserved width markers, which is why the inline range stops at 23.
        0..=23 => out.push(major | (value as u8)),
        24..=0xff => {
            out.push(major | 24);
            out.push(value as u8);
        }
        0x100..=0xffff => {
            out.push(major | 25);
            out.extend_from_slice(&(value as u16).to_be_bytes());
        }
        0x1_0000..=0xffff_ffff => {
            out.push(major | 26);
            out.extend_from_slice(&(value as u32).to_be_bytes());
        }
        _ => {
            out.push(major | 27);
            out.extend_from_slice(&value.to_be_bytes());
        }
    }
}

/// Why a byte string was not a canonical encoding.
///
/// Separate from [`CanonError`] because the two failure sets do not overlap:
/// `encode` can never run out of input and `decode` can never be handed an
/// out-of-range integer, so one enum covering both would give every caller
/// variants it cannot reach.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// The input ended in the middle of an item.
    Truncated { at: usize, wanted: usize },
    /// A major type or additional-information value the subset excludes:
    /// indefinite lengths, tags, floats, and simple values other than the
    /// three named ones.
    Unsupported { at: usize, head: u8 },
    /// An integer, length or map size encoded in a wider form than needed.
    /// Refused rather than accepted, because accepting it would give one
    /// value two encodings and content addressing would stop being a
    /// function of the value.
    NonMinimalInt { at: usize, value: u64, width: u8 },
    /// Map keys are not in strictly ascending order by encoded bytes, which
    /// is either an ordering violation or a duplicate key.
    UnsortedMapKeys { at: usize },
    /// A text string was not valid UTF-8.
    NotUtf8 { at: usize },
    /// The value nests deeper than [`MAX_DEPTH`].
    TooDeep { limit: u32 },
    /// The item ended before the input did. A canonical encoding is exactly
    /// one item, so trailing bytes mean the caller was handed something other
    /// than what it thinks it has.
    TrailingBytes { consumed: usize, total: usize },
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { at, wanted } => {
                write!(f, "input ends at byte {at}, needing {wanted} more")
            }
            Self::Unsupported { at, head } => {
                write!(
                    f,
                    "head byte 0x{head:02x} at {at} is outside the canonical subset"
                )
            }
            Self::NonMinimalInt { at, value, width } => write!(
                f,
                "{value} at byte {at} is written in {width} bytes but fits in fewer; \
                 canonical encodings use the shortest width"
            ),
            Self::UnsortedMapKeys { at } => write!(
                f,
                "map at byte {at} has keys that are not strictly ascending by encoded bytes"
            ),
            Self::NotUtf8 { at } => write!(f, "text string at byte {at} is not UTF-8"),
            Self::TooDeep { limit } => write!(f, "value nests deeper than {limit} levels"),
            Self::TrailingBytes { consumed, total } => write!(
                f,
                "item ends after {consumed} bytes but the input is {total} bytes long"
            ),
        }
    }
}

impl core::error::Error for DecodeError {}

/// Decode a canonical encoding back to its value.
///
/// This is the inverse of [`encode`] and refuses everything `encode` would not
/// have produced. That strictness is the point: `decode` accepting a
/// non-canonical spelling would mean two byte strings decode to one value,
/// and since the bytes are what gets hashed into an [`ObjId`], one value would
/// have two addresses. Round-tripping is therefore checkable in both
/// directions -- `decode(encode(v)) == v` and `encode(decode(b)) == b` -- and
/// both are tested.
///
/// [`ObjId`]: crate::ObjId
pub fn decode(bytes: &[u8]) -> Result<CanonValue, DecodeError> {
    let mut cursor = Cursor { bytes, at: 0 };
    let value = read_value(&mut cursor, 0)?;
    if cursor.at != bytes.len() {
        return Err(DecodeError::TrailingBytes {
            consumed: cursor.at,
            total: bytes.len(),
        });
    }
    Ok(value)
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], DecodeError> {
        let end = self.at.checked_add(n).ok_or(DecodeError::Truncated {
            at: self.at,
            wanted: n,
        })?;
        let slice = self.bytes.get(self.at..end).ok_or(DecodeError::Truncated {
            at: self.at,
            wanted: n,
        })?;
        self.at = end;
        Ok(slice)
    }

    fn byte(&mut self) -> Result<u8, DecodeError> {
        let slice = self.take(1)?;
        slice.first().copied().ok_or(DecodeError::Truncated {
            at: self.at,
            wanted: 1,
        })
    }
}

/// Read a head byte and its argument, rejecting any non-minimal width.
///
/// Returns the major type (already masked into the top three bits, matching
/// the `MAJOR_*` constants) and the argument.
fn read_head(cursor: &mut Cursor<'_>) -> Result<(u8, u64), DecodeError> {
    let at = cursor.at;
    let head = cursor.byte()?;
    let major = head & 0xe0;
    let info = head & 0x1f;
    let (value, width) = match info {
        0..=23 => (u64::from(info), 0u8),
        24 => (u64::from(cursor.byte()?), 1),
        25 => {
            let raw = cursor.take(2)?;
            (u64::from(u16::from_be_bytes(fixed(raw)?)), 2)
        }
        26 => {
            let raw = cursor.take(4)?;
            (u64::from(u32::from_be_bytes(fixed(raw)?)), 4)
        }
        27 => {
            let raw = cursor.take(8)?;
            (u64::from_be_bytes(fixed(raw)?), 8)
        }
        // 28..=30 are reserved and 31 is the indefinite-length marker; the
        // subset excludes all four.
        _ => return Err(DecodeError::Unsupported { at, head }),
    };
    // Minimality: the encoder would have used the narrowest form that holds
    // the value, so anything wider than necessary is not something `encode`
    // could have produced.
    let minimal_width = match value {
        0..=23 => 0u8,
        24..=0xff => 1,
        0x100..=0xffff => 2,
        0x1_0000..=0xffff_ffff => 4,
        _ => 8,
    };
    if width != minimal_width {
        return Err(DecodeError::NonMinimalInt { at, value, width });
    }
    Ok((major, value))
}

/// Copy a slice of known length into an array. The length is guaranteed by
/// the `take` above each call site; this converts that into the array type
/// without indexing.
fn fixed<const N: usize>(slice: &[u8]) -> Result<[u8; N], DecodeError> {
    <[u8; N]>::try_from(slice).map_err(|_| DecodeError::Truncated { at: 0, wanted: N })
}

fn read_value(cursor: &mut Cursor<'_>, depth: u32) -> Result<CanonValue, DecodeError> {
    if depth > MAX_DEPTH {
        return Err(DecodeError::TooDeep { limit: MAX_DEPTH });
    }
    let at = cursor.at;
    // Major 7 carries no argument in this subset, so it is read before the
    // head-argument path rather than through it: the three allowed simple
    // values live entirely in the head byte.
    if cursor
        .bytes
        .get(at)
        .is_some_and(|b| b & 0xe0 == MAJOR_SIMPLE)
    {
        let head = cursor.byte()?;
        return match head & 0x1f {
            SIMPLE_FALSE => Ok(CanonValue::Bool(false)),
            SIMPLE_TRUE => Ok(CanonValue::Bool(true)),
            SIMPLE_NULL => Ok(CanonValue::Null),
            // Includes the three float widths (25, 26, 27), which the subset
            // excludes at the type level on the encode side.
            _ => Err(DecodeError::Unsupported { at, head }),
        };
    }

    let (major, argument) = read_head(cursor)?;
    let length = usize::try_from(argument).map_err(|_| DecodeError::Truncated {
        at,
        wanted: usize::MAX,
    })?;
    match major {
        MAJOR_UINT => Ok(CanonValue::Int(i128::from(argument))),
        // Major 1 holds -1 - n, which is why the negative line reaches one
        // further than the positive one.
        MAJOR_NEGINT => Ok(CanonValue::Int(-1 - i128::from(argument))),
        MAJOR_BYTES => Ok(CanonValue::Bytes(cursor.take(length)?.to_vec())),
        MAJOR_TEXT => {
            let raw = cursor.take(length)?;
            let text = core::str::from_utf8(raw).map_err(|_| DecodeError::NotUtf8 { at })?;
            Ok(CanonValue::Str(text.to_owned()))
        }
        MAJOR_ARRAY => {
            let mut items = Vec::new();
            for _ in 0..length {
                items.push(read_value(cursor, depth + 1)?);
            }
            Ok(CanonValue::Array(items))
        }
        MAJOR_MAP => read_map(cursor, depth, length, at),
        // Every remaining major type is a tag (6), which the subset excludes.
        _ => Err(DecodeError::Unsupported {
            at,
            head: major | 0x1f,
        }),
    }
}

fn read_map(
    cursor: &mut Cursor<'_>,
    depth: u32,
    length: usize,
    at: usize,
) -> Result<CanonValue, DecodeError> {
    let mut entries = Vec::with_capacity(length.min(1024));
    let mut previous_key: Option<&[u8]> = None;
    for _ in 0..length {
        let key_start = cursor.at;
        let key = read_value(cursor, depth + 1)?;
        let key_end = cursor.at;
        // Compare the encoded key bytes as they appeared, which is the order
        // `encode` sorted by. Re-encoding the decoded key would work too but
        // would compare a value the input might not have spelled that way.
        let key_bytes = cursor
            .bytes
            .get(key_start..key_end)
            .ok_or(DecodeError::Truncated {
                at: key_start,
                wanted: key_end - key_start,
            })?;
        // Strictly ascending, so this rejects duplicates and misordering with
        // one comparison: equal keys are not ascending.
        if previous_key.is_some_and(|last| last >= key_bytes) {
            return Err(DecodeError::UnsortedMapKeys { at });
        }
        previous_key = Some(key_bytes);
        let value = read_value(cursor, depth + 1)?;
        entries.push((key, value));
    }
    Ok(CanonValue::Map(entries))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn scalars_match_rfc_8949_examples() -> Result<(), CanonError> {
        assert_eq!(hex(&encode(&CanonValue::Null)?), "f6");
        assert_eq!(hex(&encode(&CanonValue::Bool(false))?), "f4");
        assert_eq!(hex(&encode(&CanonValue::Bool(true))?), "f5");
        assert_eq!(hex(&encode(&CanonValue::Int(0))?), "00");
        assert_eq!(hex(&encode(&CanonValue::Int(23))?), "17");
        assert_eq!(hex(&encode(&CanonValue::Int(24))?), "1818");
        assert_eq!(hex(&encode(&CanonValue::Int(1000))?), "1903e8");
        assert_eq!(hex(&encode(&CanonValue::Int(-1))?), "20");
        assert_eq!(hex(&encode(&CanonValue::Int(-500))?), "3901f3");
        assert_eq!(hex(&encode(&CanonValue::str("a"))?), "6161");
        assert_eq!(hex(&encode(&CanonValue::Bytes(vec![1, 2]))?), "420102");
        Ok(())
    }

    #[test]
    fn integers_use_the_shortest_width() -> Result<(), CanonError> {
        // One byte of head per step up, and no wasted width in between.
        for (value, len) in [(23i128, 1), (24, 2), (0xff, 2), (0x100, 3), (0x1_0000, 5)] {
            assert_eq!(encode(&CanonValue::Int(value))?.len(), len, "value {value}");
        }
        Ok(())
    }

    #[test]
    fn integer_range_is_the_full_cbor_line() {
        let top = i128::from(u64::MAX);
        assert!(encode(&CanonValue::Int(top)).is_ok());
        assert!(encode(&CanonValue::Int(-1 - top)).is_ok());
        assert_eq!(
            encode(&CanonValue::Int(top + 1)),
            Err(CanonError::IntOutOfRange { value: top + 1 })
        );
    }

    #[test]
    fn map_order_does_not_depend_on_construction_order() -> Result<(), CanonError> {
        let one = CanonValue::map([("b", CanonValue::Int(2)), ("a", CanonValue::Int(1))]);
        let other = CanonValue::map([("a", CanonValue::Int(1)), ("b", CanonValue::Int(2))]);
        assert_eq!(encode(&one)?, encode(&other)?);
        assert_eq!(hex(&encode(&one)?), "a2616101616202");
        Ok(())
    }

    /// Bytewise order over encoded keys, not lexicographic order over the
    /// decoded strings: "z" sorts before "aa" because its encoding is shorter
    /// and CBOR heads make length the leading byte.
    #[test]
    fn short_keys_sort_before_long_ones() -> Result<(), CanonError> {
        let encoded = encode(&CanonValue::map([
            ("aa", CanonValue::Null),
            ("z", CanonValue::Null),
        ]))?;
        assert_eq!(hex(&encoded), "a2617af6626161f6");
        Ok(())
    }

    #[test]
    fn duplicate_keys_are_refused() {
        let value = CanonValue::map([("a", CanonValue::Int(1)), ("a", CanonValue::Int(2))]);
        assert!(matches!(
            encode(&value),
            Err(CanonError::DuplicateKey { .. })
        ));
    }

    /// The property the memo table depends on. Distinct values that a sloppier
    /// encoder would confuse must stay distinct here.
    #[test]
    fn structurally_distinct_values_do_not_collide() -> Result<(), CanonError> {
        let candidates = [
            CanonValue::Null,
            CanonValue::Bool(false),
            CanonValue::Int(0),
            CanonValue::Bytes(vec![0x61]),
            CanonValue::str("a"),
            CanonValue::array([CanonValue::str("a")]),
            CanonValue::map([("a", CanonValue::Null)]),
            CanonValue::array([CanonValue::str("ab")]),
            CanonValue::array([CanonValue::str("a"), CanonValue::str("b")]),
        ];
        let mut seen: std::collections::BTreeSet<Vec<u8>> = std::collections::BTreeSet::new();
        for candidate in &candidates {
            assert!(
                seen.insert(encode(candidate)?),
                "collision on {candidate:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn depth_is_capped() {
        let mut value = CanonValue::Null;
        for _ in 0..=MAX_DEPTH {
            value = CanonValue::array([value]);
        }
        assert_eq!(
            encode(&value),
            Err(CanonError::TooDeep { limit: MAX_DEPTH })
        );
    }

    /// A sample spanning every variant and both integer signs, reused by the
    /// round-trip tests below.
    fn samples() -> Vec<CanonValue> {
        vec![
            CanonValue::Null,
            CanonValue::Bool(true),
            CanonValue::Bool(false),
            CanonValue::Int(0),
            CanonValue::Int(23),
            CanonValue::Int(24),
            CanonValue::Int(1000),
            CanonValue::Int(-1),
            CanonValue::Int(-500),
            CanonValue::Int(i128::from(u64::MAX)),
            CanonValue::Int(-1 - i128::from(u64::MAX)),
            CanonValue::Bytes(Vec::new()),
            CanonValue::Bytes(vec![0, 1, 2, 0xff]),
            CanonValue::str(""),
            CanonValue::str("hello"),
            CanonValue::str("nix \u{2400} unicode"),
            CanonValue::array([]),
            CanonValue::array([CanonValue::Int(1), CanonValue::str("a")]),
            CanonValue::Map(Vec::new()),
            CanonValue::map([("b", CanonValue::Int(2)), ("a", CanonValue::Null)]),
            CanonValue::map([(
                "nested",
                CanonValue::array([CanonValue::map([("deep", CanonValue::Bool(true))])]),
            )]),
        ]
    }

    #[test]
    fn decode_inverts_encode() -> Result<(), Box<dyn core::error::Error>> {
        for value in samples() {
            let bytes = encode(&value)?;
            let back = decode(&bytes)?;
            // Maps compare by construction order, and `encode` sorts, so the
            // decoded map is the sorted spelling. Comparing re-encoded bytes
            // is the equality that matters for addressing.
            assert_eq!(encode(&back)?, bytes, "value {value:?}");
        }
        Ok(())
    }

    /// The direction that matters for content addressing: a byte string
    /// `decode` accepts must re-encode to itself, or one value would have two
    /// addresses.
    #[test]
    fn accepted_bytes_re_encode_to_themselves() -> Result<(), Box<dyn core::error::Error>> {
        for value in samples() {
            let bytes = encode(&value)?;
            assert_eq!(encode(&decode(&bytes)?)?, bytes);
        }
        Ok(())
    }

    #[test]
    fn decode_recovers_scalars_exactly() -> Result<(), Box<dyn core::error::Error>> {
        assert_eq!(
            decode(&encode(&CanonValue::Int(-500))?)?,
            CanonValue::Int(-500)
        );
        assert_eq!(
            decode(&encode(&CanonValue::str("a"))?)?,
            CanonValue::str("a")
        );
        assert_eq!(decode(&[0xf6])?, CanonValue::Null);
        assert_eq!(decode(&[0xf5])?, CanonValue::Bool(true));
        assert_eq!(decode(&[0xf4])?, CanonValue::Bool(false));
        Ok(())
    }

    /// 0x1817 is 23 written in two bytes. The encoder writes 23 as 0x17, so
    /// accepting the wide form would give one integer two addresses.
    #[test]
    fn non_minimal_integers_are_refused() {
        assert_eq!(
            decode(&[0x18, 0x17]),
            Err(DecodeError::NonMinimalInt {
                at: 0,
                value: 23,
                width: 1
            })
        );
        // 0x1900ff is 255 in two bytes; minimal is 0x18ff.
        assert_eq!(
            decode(&[0x19, 0x00, 0xff]),
            Err(DecodeError::NonMinimalInt {
                at: 0,
                value: 255,
                width: 2
            })
        );
    }

    /// Indefinite-length items (additional info 31) end at a break byte
    /// rather than a declared length, so their extent is not known from the
    /// head. The subset excludes them.
    #[test]
    fn indefinite_lengths_are_refused() {
        // 0x9f is an indefinite-length array.
        assert!(matches!(
            decode(&[0x9f, 0x01, 0xff]),
            Err(DecodeError::Unsupported { .. })
        ));
        // 0x5f, indefinite-length byte string.
        assert!(matches!(
            decode(&[0x5f, 0xff]),
            Err(DecodeError::Unsupported { .. })
        ));
    }

    /// Floats are excluded at the type level on the encode side, so the
    /// decoder must refuse them rather than inventing a variant to hold one.
    #[test]
    fn floats_are_refused() {
        // 0xf9 half, 0xfa single, 0xfb double.
        for head in [0xf9u8, 0xfa, 0xfb] {
            let bytes = [head, 0, 0, 0, 0, 0, 0, 0, 0];
            assert!(
                matches!(decode(&bytes), Err(DecodeError::Unsupported { .. })),
                "head {head:02x} was accepted"
            );
        }
    }

    #[test]
    fn tags_are_refused() {
        // 0xc0, tag 0 followed by a text string.
        assert!(matches!(
            decode(&[0xc0, 0x61, 0x61]),
            Err(DecodeError::Unsupported { .. })
        ));
    }

    /// `encode` sorts map keys, so an unsorted map is not something it could
    /// have produced, and accepting one would give the same map two spellings.
    #[test]
    fn unsorted_map_keys_are_refused() {
        // {"b": 2, "a": 1} in that order: 0xa2 map(2), then "b", 2, "a", 1.
        let bytes = [0xa2, 0x61, 0x62, 0x02, 0x61, 0x61, 0x01];
        assert_eq!(decode(&bytes), Err(DecodeError::UnsortedMapKeys { at: 0 }));
    }

    /// Equal keys are not strictly ascending, so the same comparison catches
    /// duplicates without a second pass.
    #[test]
    fn duplicate_map_keys_are_refused() {
        let bytes = [0xa2, 0x61, 0x61, 0x01, 0x61, 0x61, 0x02];
        assert_eq!(decode(&bytes), Err(DecodeError::UnsortedMapKeys { at: 0 }));
    }

    #[test]
    fn trailing_bytes_are_refused() {
        assert_eq!(
            decode(&[0xf6, 0xf6]),
            Err(DecodeError::TrailingBytes {
                consumed: 1,
                total: 2
            })
        );
    }

    #[test]
    fn truncated_input_is_refused() {
        // Declares a 4-byte text string and supplies one byte.
        assert!(matches!(
            decode(&[0x64, 0x61]),
            Err(DecodeError::Truncated { .. })
        ));
        // Declares a 2-element array and supplies one.
        assert!(matches!(
            decode(&[0x82, 0x01]),
            Err(DecodeError::Truncated { .. })
        ));
        assert!(matches!(decode(&[]), Err(DecodeError::Truncated { .. })));
    }

    #[test]
    fn non_utf8_text_is_refused() {
        // 0x62 is a 2-byte text string; 0xff 0xfe is not UTF-8.
        assert_eq!(
            decode(&[0x62, 0xff, 0xfe]),
            Err(DecodeError::NotUtf8 { at: 0 })
        );
    }

    #[test]
    fn decode_depth_is_capped() -> Result<(), CanonError> {
        // One level under the cap encodes, so the refusal below is the depth
        // check and not an encoding failure.
        let mut value = CanonValue::Null;
        for _ in 0..MAX_DEPTH {
            value = CanonValue::array([value]);
        }
        let bytes = encode(&value)?;
        assert!(decode(&bytes).is_ok());
        // A hand-built array nest one level deeper than the encoder allows.
        let mut deeper = vec![0x81u8; usize::try_from(MAX_DEPTH).unwrap_or(usize::MAX) + 1];
        deeper.push(0xf6);
        assert_eq!(
            decode(&deeper),
            Err(DecodeError::TooDeep { limit: MAX_DEPTH })
        );
        Ok(())
    }
}
