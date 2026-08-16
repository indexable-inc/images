//! Parsing the hash a fixed-output derivation declares, as cppnix's
//! `libutil/hash.cc` does it.
//!
//! Three encodings reach `outputHash` from nixpkgs -- base16, nix32 and SRI --
//! and which one a string is in is decided by its *length*, not by its
//! characters, so a hash of the wrong length is a length error rather than a
//! character error. That rule and the algorithm-resolution rule above it are
//! the whole of this module; the bytes it produces go straight into a store
//! path, so being one byte or one branch off is a wrong path that looks
//! exactly like a right one.

/// The four algorithms this backend accepts. cppnix's `parseHashAlgoOpt` also
/// knows `blake3`, behind an experimental feature; see [`parse_algo_opt`].
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum HashAlgo {
    Md5,
    Sha1,
    Sha256,
    Sha512,
}

impl HashAlgo {
    /// `regularHashSize` (`libutil/hash.cc`), in bytes.
    #[must_use]
    pub fn size(self) -> usize {
        match self {
            HashAlgo::Md5 => 16,
            HashAlgo::Sha1 => 20,
            HashAlgo::Sha256 => 32,
            HashAlgo::Sha512 => 64,
        }
    }

    /// `printHashAlgo`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            HashAlgo::Md5 => "md5",
            HashAlgo::Sha1 => "sha1",
            HashAlgo::Sha256 => "sha256",
            HashAlgo::Sha512 => "sha512",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HashError {
    /// cppnix's `parseHashAlgo` on an unrecognised prefix inside the hash
    /// string. Note this is *not* what an unrecognised `outputHashAlgo`
    /// attribute produces: see [`parse_algo_opt`].
    UnknownAlgo(String),
    /// No algorithm in the string and none from the attribute.
    NoType(String),
    /// The string names one algorithm and `outputHashAlgo` names another.
    WrongType { hash: String, want: HashAlgo },
    /// The encoded length matches none of base16, nix32 or base64.
    WrongLength { hash: String, algo: HashAlgo },
    /// A character no digit of the chosen encoding can be.
    BadChar { encoding: &'static str, ch: char },
    /// Decoded, but to the wrong number of bytes. Reachable from nix32 and
    /// base64, whose encoded length does not pin the decoded length.
    BadDecodedLength {
        encoding: &'static str,
        hash: String,
        got: usize,
        want: usize,
    },
    /// `newHashAllowEmpty` with neither a hash nor an algorithm.
    EmptyWithoutAlgo,
    /// Recognised by cppnix, gated behind an experimental feature this
    /// backend does not implement. Reported as a refusal, never as a Nix
    /// error, because cppnix with the feature on answers fine.
    Unsupported(&'static str),
    /// cppnix's `parseHashFormat` on a name that is none of the five.
    UnknownFormat(String),
}

impl core::fmt::Display for HashError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            HashError::UnknownAlgo(s) => write!(
                f,
                "unknown hash algorithm '{s}', expect 'blake3', 'md5', 'sha1', 'sha256', or 'sha512'"
            ),
            HashError::NoType(s) => write!(
                f,
                "hash '{s}' does not include a type, nor is the type otherwise known from context"
            ),
            HashError::WrongType { hash, want } => {
                write!(f, "hash '{hash}' should have type '{}'", want.name())
            }
            HashError::WrongLength { hash, algo } => write!(
                f,
                "hash '{hash}' has wrong length for hash algorithm '{}'",
                algo.name()
            ),
            HashError::BadChar { encoding, ch } => {
                write!(f, "invalid character in {encoding} string: '{ch}'")
            }
            HashError::BadDecodedLength {
                encoding,
                hash,
                got,
                want,
            } => write!(
                f,
                "invalid {encoding} hash '{hash}', length {got} != expected length {want}"
            ),
            HashError::EmptyWithoutAlgo => write!(f, "empty hash requires explicit hash algorithm"),
            HashError::UnknownFormat(s) => write!(
                f,
                "unknown hash format '{s}', expect 'base16', 'base32', 'base64', or 'sri'"
            ),
            HashError::Unsupported(what) => write!(f, "{what}"),
        }
    }
}

impl core::error::Error for HashError {}

type Result<T> = core::result::Result<T, HashError>;

/// A parsed hash: an algorithm and exactly `algo.size()` bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hash {
    pub algo: HashAlgo,
    pub bytes: Vec<u8>,
}

impl Hash {
    /// The all-zero hash `newHashAllowEmpty` substitutes for an empty string.
    #[must_use]
    pub fn zero(algo: HashAlgo) -> Hash {
        Hash {
            algo,
            bytes: vec![0u8; algo.size()],
        }
    }

    /// `to_string(HashFormat::Base16, include_algo)`.
    #[must_use]
    pub fn to_base16(&self, include_algo: bool) -> String {
        self.to_format(HashFormat::Base16, include_algo)
    }

    /// `to_string(HashFormat::SRI, true)`, which is the form cppnix's
    /// empty-hash warning prints.
    #[must_use]
    pub fn to_sri(&self) -> String {
        self.to_format(HashFormat::Sri, true)
    }

    /// `Hash::to_string(hashFormat, includeAlgo)`: SRI always carries the
    /// algorithm with a `-`, the other three carry `algo:` only when asked.
    #[must_use]
    pub fn to_format(&self, format: HashFormat, include_algo: bool) -> String {
        let mut out = String::new();
        if format == HashFormat::Sri || include_algo {
            out.push_str(self.algo.name());
            out.push(if format == HashFormat::Sri { '-' } else { ':' });
        }
        match format {
            HashFormat::Base16 => {
                for b in &self.bytes {
                    let _ = core::fmt::Write::write_fmt(&mut out, format_args!("{b:02x}"));
                }
            }
            HashFormat::Nix32 => out.push_str(&crate::drvpath::nix32_encode(&self.bytes)),
            HashFormat::Base64 | HashFormat::Sri => out.push_str(&base64_encode(&self.bytes)),
        }
        out
    }
}

/// `HashFormat`: the four renderings a parsed hash can be asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashFormat {
    Base16,
    Nix32,
    Base64,
    Sri,
}

/// `parseHashFormat`, including the deprecation `parseHashFormatOpt` warns
/// about: "base32" still parses (as nix32) and the exact cppnix warning line
/// comes back for the caller to emit, because this crate performs no IO and
/// the embedder owns the logger.
pub fn parse_hash_format(name: &str) -> Result<(HashFormat, Option<String>)> {
    match name {
        "base16" => Ok((HashFormat::Base16, None)),
        "nix32" => Ok((HashFormat::Nix32, None)),
        "base32" => Ok((
            HashFormat::Nix32,
            Some(r#""base32" is a deprecated alias for hash format "nix32"."#.to_owned()),
        )),
        "base64" => Ok((HashFormat::Base64, None)),
        "sri" => Ok((HashFormat::Sri, None)),
        _ => Err(HashError::UnknownFormat(name.to_owned())),
    }
}

/// `parseHashAlgoOpt`: an unrecognised name is **not** an error, it is
/// `None`, and the caller then needs the algorithm from the hash string
/// itself. Only `blake3` is special, because cppnix recognises it and then
/// requires an experimental feature; refusing by name keeps that a refusal
/// rather than a wrong answer.
pub fn parse_algo_opt(s: &str) -> Result<Option<HashAlgo>> {
    match s {
        "md5" => Ok(Some(HashAlgo::Md5)),
        "sha1" => Ok(Some(HashAlgo::Sha1)),
        "sha256" => Ok(Some(HashAlgo::Sha256)),
        "sha512" => Ok(Some(HashAlgo::Sha512)),
        "blake3" => Err(HashError::Unsupported(
            "blake3 hashes (cppnix gates these behind the blake3-hashes experimental feature)",
        )),
        _ => Ok(None),
    }
}

/// `parseHashAlgo`: used for the prefix *inside* a hash string, where an
/// unrecognised name is an error.
pub(crate) fn parse_algo(s: &str) -> Result<HashAlgo> {
    parse_algo_opt(s)?.ok_or_else(|| HashError::UnknownAlgo(s.to_owned()))
}

/// `Hash::parseAny`. `opt_algo` is what `outputHashAlgo` said, if anything.
pub fn parse_any(original: &str, opt_algo: Option<HashAlgo>) -> Result<Hash> {
    // `splitPrefixTo(rest, ':')`, then `'-'` for SRI. A `:` anywhere wins over
    // a `-`, which is why these are tried in this order and not by shape.
    let (parsed, rest, is_sri) = if let Some((prefix, rest)) = original.split_once(':') {
        (Some(parse_algo(prefix)?), rest, false)
    } else if let Some((prefix, rest)) = original.split_once('-') {
        (Some(parse_algo(prefix)?), rest, true)
    } else {
        (None, original, false)
    };

    let algo = match (parsed, opt_algo) {
        (None, None) => return Err(HashError::NoType(original.to_owned())),
        (Some(p), Some(o)) if p != o => {
            return Err(HashError::WrongType {
                hash: original.to_owned(),
                want: o,
            });
        }
        (Some(p), _) => p,
        (None, Some(o)) => o,
    };

    // SRI is always base64; otherwise the encoding is deduced from the length,
    // which is why a mistyped hash reports a length error and not a bad
    // character.
    let (bytes, encoding) = if is_sri {
        (base64_decode(rest)?, "SRI")
    } else if rest.len() == algo.size() * 2 {
        (base16_decode(rest)?, "base16")
    } else if rest.len() == nix32_len(algo.size()) {
        (nix32_decode(rest)?, "nix32")
    } else if rest.len() == base64_len(algo.size()) {
        (base64_decode(rest)?, "Base64")
    } else {
        return Err(HashError::WrongLength {
            hash: rest.to_owned(),
            algo,
        });
    };

    if bytes.len() != algo.size() {
        return Err(HashError::BadDecodedLength {
            encoding,
            hash: rest.to_owned(),
            got: bytes.len(),
            want: algo.size(),
        });
    }
    Ok(Hash { algo, bytes })
}

/// `newHashAllowEmpty`. An empty hash is accepted and becomes the all-zero
/// hash of the declared algorithm, which cppnix *warns* about; the warning is
/// returned rather than printed, because this crate performs no IO and the
/// embedder owns the logger.
pub fn new_hash_allow_empty(
    hash: &str,
    opt_algo: Option<HashAlgo>,
) -> Result<(Hash, Option<String>)> {
    if hash.is_empty() {
        let algo = opt_algo.ok_or(HashError::EmptyWithoutAlgo)?;
        let h = Hash::zero(algo);
        let warning = format!("found empty hash, assuming '{}'", h.to_sri());
        return Ok((h, Some(warning)));
    }
    Ok((parse_any(hash, opt_algo)?, None))
}

/// `BaseNix32::encodedLength`.
fn nix32_len(size: usize) -> usize {
    if size == 0 { 0 } else { (size * 8 - 1) / 5 + 1 }
}

/// `base64::encodedLength`: `((4 * n / 3) + 3) & ~3`.
fn base64_len(size: usize) -> usize {
    ((4 * size / 3) + 3) & !3
}

fn base16_decode(s: &str) -> Result<Vec<u8>> {
    let digit = |c: u8| -> Result<u8> {
        match c {
            b'0'..=b'9' => Ok(c - b'0'),
            b'A'..=b'F' => Ok(c - b'A' + 10),
            b'a'..=b'f' => Ok(c - b'a' + 10),
            _ => Err(HashError::BadChar {
                encoding: "Base16",
                ch: char::from(c),
            }),
        }
    };
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        let (hi, lo) = match (pair.first(), pair.get(1)) {
            (Some(a), Some(b)) => (*a, *b),
            _ => break,
        };
        out.push((digit(hi)? << 4) | digit(lo)?);
    }
    Ok(out)
}

const NIX32_CHARS: &[u8; 32] = b"0123456789abcdfghijklmnpqrsvwxyz";

/// `BaseNix32::decode` (`libutil/base-nix-32.cc:42`), transcribed: it walks the
/// input from the *end*, so writing it the natural way decodes to different
/// bytes. The high half can push the output one byte past the expected length,
/// which is a real case and is caught by the length check in [`parse_any`].
fn nix32_decode(s: &str) -> Result<Vec<u8>> {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity((bytes.len() * 5).div_ceil(8));
    for n in 0..bytes.len() {
        let c = *bytes.get(bytes.len() - n - 1).ok_or(HashError::BadChar {
            encoding: "Nix32",
            ch: '?',
        })?;
        let digit = NIX32_CHARS
            .iter()
            .position(|d| *d == c)
            .ok_or(HashError::BadChar {
                encoding: "Nix32 (Nix's Base32 variation)",
                ch: char::from(c),
            })?;
        let digit = u32::try_from(digit).unwrap_or(0);
        let b = n * 5;
        let i = b / 8;
        let j = u32::try_from(b % 8).unwrap_or(0);
        if out.len() < i + 1 {
            out.resize(i + 1, 0);
        }
        if let Some(slot) = out.get_mut(i) {
            *slot |= u8::try_from((digit << j) & 0xff).unwrap_or(0);
        }
        // `digit >> (8 - j)` with `j == 0` is a shift by 8. cppnix promotes to
        // `int` and gets 0; a `u8` shift would panic, so this is done in u32.
        let carry = digit >> (8 - j);
        if carry != 0 {
            if out.len() < i + 2 {
                out.resize(i + 2, 0);
            }
            if let Some(slot) = out.get_mut(i + 1) {
                *slot |= u8::try_from(carry & 0xff).unwrap_or(0);
            }
        }
    }
    Ok(out)
}

const BASE64_CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    let mut data: u32 = 0;
    let mut nbits: u32 = 0;
    for c in bytes {
        data = (data << 8) | u32::from(*c);
        nbits += 8;
        while nbits >= 6 {
            nbits -= 6;
            let idx = usize::try_from((data >> nbits) & 0x3f).unwrap_or(0);
            out.push(char::from(*BASE64_CHARS.get(idx).unwrap_or(&b'A')));
        }
    }
    if nbits != 0 {
        let idx = usize::try_from((data << (6 - nbits)) & 0x3f).unwrap_or(0);
        out.push(char::from(*BASE64_CHARS.get(idx).unwrap_or(&b'A')));
    }
    while !out.len().is_multiple_of(4) {
        out.push('=');
    }
    out
}

/// `base64::decode`. Stops at the first `=` and skips newlines, as cppnix
/// does; a short sequence is therefore not an error here, and the decoded
/// length check in [`parse_any`] is what refuses it.
fn base64_decode(s: &str) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len().div_ceil(4) * 3);
    let mut d: u32 = 0;
    let mut bits: u32 = 0;
    for c in s.as_bytes() {
        if *c == b'=' {
            break;
        }
        if *c == b'\n' {
            continue;
        }
        let digit = BASE64_CHARS
            .iter()
            .position(|x| x == c)
            .ok_or(HashError::BadChar {
                encoding: "Base64",
                ch: char::from(*c),
            })?;
        bits += 6;
        d = (d << 6) | u32::try_from(digit).unwrap_or(0);
        if bits >= 8 {
            out.push(u8::try_from((d >> (bits - 8)) & 0xff).unwrap_or(0));
            bits -= 8;
        }
    }
    Ok(out)
}

/// This module had no test of its own until mutation testing was run over it
/// (ENG-13020). It was not uncovered -- `drvstrict`'s fixed-output tests parse
/// hashes through it and killed 152 of its 155 mutants -- but what they reach
/// is the sha256 path, because that is what every fixed-output fixture in this
/// crate declares. The tests below are the cases that indirect coverage cannot
/// reach.
#[cfg(test)]
mod tests {
    use super::{HashAlgo, HashError, base64_len, nix32_len, parse_algo_opt, parse_any};

    /// The encoded length of a nix32 digest, for each algorithm this accepts.
    ///
    /// `nix32_len` is `(size * 8 - 1) / 5 + 1`, and the `- 1` matters for
    /// exactly one of the four sizes: at 16, 32 and 64 bytes the subtraction
    /// falls inside the same five-bit window and dropping it changes nothing,
    /// while at sha1's 20 bytes it is the difference between 32 characters and
    /// 33. So a suite that only ever parses sha256 cannot see that term at all.
    ///
    /// The expected values are cppnix's: `nix hash convert --to nix32` emits
    /// 32 characters for a sha1 digest.
    #[test]
    fn the_nix32_length_of_every_algorithm_is_cppnixs() {
        assert_eq!(nix32_len(HashAlgo::Md5.size()), 26);
        assert_eq!(nix32_len(HashAlgo::Sha1.size()), 32);
        assert_eq!(nix32_len(HashAlgo::Sha256.size()), 52);
        assert_eq!(nix32_len(HashAlgo::Sha512.size()), 103);
        // The degenerate case the formula guards with its `if`.
        assert_eq!(nix32_len(0), 0);
    }

    /// A sha1 hash in nix32, which is the length no other algorithm shares.
    ///
    /// Ground truth is cppnix:
    /// `nix hash convert --hash-algo sha1 --to nix32 a9993e36...` prints
    /// `kpcd173cq987hw957sx6m0868wv3x6d9`, the digest of "abc". Parsing it
    /// back has to give those 20 bytes; a length rule that is one out rejects
    /// this string as `WrongLength` and every sha1 `outputHash` in nixpkgs
    /// with it.
    #[test]
    fn a_sha1_hash_in_nix32_round_trips_through_cppnixs_length_rule() {
        let expected: Vec<u8> = vec![
            0xa9, 0x99, 0x3e, 0x36, 0x47, 0x06, 0x81, 0x6a, 0xba, 0x3e, 0x25, 0x71, 0x78, 0x50,
            0xc2, 0x6c, 0x9c, 0xd0, 0xd8, 0x9d,
        ];
        let got = parse_any("kpcd173cq987hw957sx6m0868wv3x6d9", Some(HashAlgo::Sha1));
        assert_eq!(got.as_ref().map(|h| h.algo), Ok(HashAlgo::Sha1));
        assert_eq!(got.map(|h| h.bytes), Ok(expected.clone()));

        // The same digest in the other two encodings must reach the same
        // bytes: the encoding is chosen by length, so all three are one
        // decision and getting any of the three lengths wrong silently
        // reroutes a hash into the wrong decoder.
        let base16 = parse_any(
            "a9993e364706816aba3e25717850c26c9cd0d89d",
            Some(HashAlgo::Sha1),
        );
        assert_eq!(base16.map(|h| h.bytes), Ok(expected.clone()));
        let sri = parse_any("sha1-qZk+NkcGgWq6PiVxeFDCbJzQ2J0=", None);
        assert_eq!(sri.map(|h| h.bytes), Ok(expected));
    }

    /// The base64 encoded length, which is the third arm of the same choice.
    #[test]
    fn the_base64_length_of_every_algorithm_is_cppnixs() {
        assert_eq!(base64_len(HashAlgo::Md5.size()), 24);
        assert_eq!(base64_len(HashAlgo::Sha1.size()), 28);
        assert_eq!(base64_len(HashAlgo::Sha256.size()), 44);
        assert_eq!(base64_len(HashAlgo::Sha512.size()), 88);
    }

    /// A string of no recognised length is a *length* error, naming the
    /// algorithm, rather than a character error.
    ///
    /// This is the module header's central claim -- which encoding a string is
    /// in is decided by its length, not its characters -- and it is what makes
    /// a mistyped hash report something a user can act on.
    #[test]
    fn an_unrecognised_length_is_a_length_error_not_a_character_error() {
        let got = parse_any("abc", Some(HashAlgo::Sha256));
        assert_eq!(
            got,
            Err(HashError::WrongLength {
                hash: "abc".to_owned(),
                algo: HashAlgo::Sha256,
            })
        );
    }

    /// `parseHashAlgoOpt`: an unrecognised name is `None`, not an error, and
    /// `blake3` is a refusal rather than either.
    ///
    /// The three outcomes are genuinely different downstream: `None` sends the
    /// caller to the hash string for the algorithm, an error refuses the
    /// derivation, and the refusal carries a token. cppnix accepts `blake3`
    /// behind an experimental feature, so answering `None` for it would make
    /// this backend compute a path where cppnix computes a different one.
    #[test]
    fn an_unknown_algorithm_name_is_none_and_blake3_is_a_refusal() {
        assert_eq!(parse_algo_opt("sha256"), Ok(Some(HashAlgo::Sha256)));
        assert_eq!(parse_algo_opt("md5"), Ok(Some(HashAlgo::Md5)));
        assert_eq!(parse_algo_opt("sha1"), Ok(Some(HashAlgo::Sha1)));
        assert_eq!(parse_algo_opt("sha512"), Ok(Some(HashAlgo::Sha512)));
        assert_eq!(parse_algo_opt("not-a-hash"), Ok(None));
        assert!(matches!(
            parse_algo_opt("blake3"),
            Err(HashError::Unsupported(_))
        ));
    }
}
