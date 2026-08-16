//! One error type for the kernel.
//!
//! Every variant that reports a refusal names the row it refused, because the
//! first thing anyone does with "the pin is missing" or "the hash did not
//! match" is go and look at that row. An error that makes the reader run a
//! second query to find out what it was talking about is half an error.

use crate::canon::CanonError;
use crate::hash::{Hash, HexError};
use crate::id::{Domain, Key, ObjId};
use core::fmt;

pub type Result<T> = core::result::Result<T, KernelError>;

/// A `Checked` effect produced something other than what was declared.
///
/// Split out and boxed so [`KernelError`] stays small: four hashes is 128
/// bytes, and every `Result` in the crate pays for the largest variant whether
/// or not it ever fails.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HashMismatch {
    pub domain: Domain,
    pub key: Key,
    pub declared: Hash,
    pub actual: ObjId,
}

/// Two lock files pin one key to two different outputs. Boxed for the same
/// reason as [`HashMismatch`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockConflict {
    pub domain: Domain,
    pub key: Key,
    pub ours: ObjId,
    pub theirs: ObjId,
}

#[derive(Debug)]
pub enum KernelError {
    /// A request could not be canonically encoded, so it has no key.
    Canon(CanonError),
    /// A hash arrived as text and was not a hash.
    Hex { field: String, source: HexError },
    /// Talking to a directory-backed store failed.
    Io {
        doing: String,
        source: std::io::Error,
    },
    /// A `Checked` effect produced something other than what was declared.
    /// Hard-fails rather than falling back: a declaration that can be
    /// overridden by whatever the effect happened to return is decoration.
    HashMismatch(Box<HashMismatch>),
    /// A `Pinned` effect missed the table while the kernel was frozen.
    FrozenPin { domain: Domain, key: Key },
    /// A `Transparent` effect returned a value. Transparent means the effect
    /// is observable but has no result; returning bytes means the caller
    /// expects them to be memoised, and they never will be.
    TransparentNotUnit { domain: Domain, len: usize },
    /// Two lock files pin the same key to different outputs. Not resolvable
    /// here: picking a side would silently discard somebody's audited answer.
    LockConflict(Box<LockConflict>),
    /// `effect.lock` did not parse, or parsed into the wrong shape.
    LockFormat { detail: String },
    /// The caller's effect failed. Its own error is flattened to text at the
    /// boundary so the kernel does not become generic over every effect's
    /// error type.
    Perform { domain: Domain, detail: String },
}

impl fmt::Display for KernelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Canon(source) => write!(f, "cannot encode request: {source}"),
            Self::Hex { field, source } => write!(f, "bad hash in {field}: {source}"),
            Self::Io { doing, source } => write!(f, "{doing}: {source}"),
            Self::HashMismatch(detail) => write!(
                f,
                "checked effect in domain {} key {} produced {}, but {} was declared",
                detail.domain, detail.key, detail.actual, detail.declared
            ),
            Self::FrozenPin { domain, key } => write!(
                f,
                "no pin for domain {domain} key {key}, and the kernel is frozen; \
                 run unfrozen to record one, or add the row to effect.lock"
            ),
            Self::TransparentNotUnit { domain, len } => write!(
                f,
                "transparent effect in domain {domain} returned {len} bytes; \
                 transparent effects have no result and are never memoised"
            ),
            Self::LockConflict(detail) => write!(
                f,
                "conflicting pins for domain {} key {}: {} and {}",
                detail.domain, detail.key, detail.ours, detail.theirs
            ),
            Self::LockFormat { detail } => write!(f, "malformed effect.lock: {detail}"),
            Self::Perform { domain, detail } => {
                write!(f, "effect in domain {domain} failed: {detail}")
            }
        }
    }
}

impl core::error::Error for KernelError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Canon(source) => Some(source),
            Self::Hex { source, .. } => Some(source),
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<CanonError> for KernelError {
    fn from(source: CanonError) -> Self {
        Self::Canon(source)
    }
}

impl KernelError {
    pub(crate) fn io(doing: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            doing: doing.into(),
            source,
        }
    }

    pub(crate) fn hex(field: impl Into<String>, source: HexError) -> Self {
        Self::Hex {
            field: field.into(),
            source,
        }
    }

    pub(crate) fn lock(detail: impl Into<String>) -> Self {
        Self::LockFormat {
            detail: detail.into(),
        }
    }

    pub(crate) fn mismatch(domain: Domain, key: Key, declared: Hash, actual: ObjId) -> Self {
        Self::HashMismatch(Box::new(HashMismatch {
            domain,
            key,
            declared,
            actual,
        }))
    }

    pub(crate) fn conflict(domain: Domain, key: Key, ours: ObjId, theirs: ObjId) -> Self {
        Self::LockConflict(Box::new(LockConflict {
            domain,
            key,
            ours,
            theirs,
        }))
    }
}
