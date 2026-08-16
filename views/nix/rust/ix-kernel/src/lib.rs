//! The effects kernel: one memo table over one content-addressed store, and
//! four trust policies over that pair.
//!
//! The claim this crate exists to make good on is that the store, the
//! evaluation cache and `effect.lock` are not three mechanisms. They are one
//! table, `(Domain, Key) -> ObjId`, read under three different disciplines:
//!
//! | discipline | what it assumes | on a miss |
//! |---|---|---|
//! | [`Policy::Keyed`] | the effect is a function of its request | perform, record |
//! | [`Policy::Checked`] | the answer is declared in advance | perform once, verify, record |
//! | [`Policy::Pinned`] | there is no reproducing it | record the first answer, or refuse if frozen |
//! | [`Policy::Transparent`] | the effect has no value | perform, record nothing |
//!
//! Collapsing them onto one table is what makes the guarantees comparable: a
//! row's [`Provenance`] says exactly how much of the build is reproducible and
//! how much rests on somebody having said so once.
//!
//! # Shape of a call
//!
//! ```
//! use ix_kernel::{
//!     canon::{self, CanonValue},
//!     cas::MemoryCas,
//!     dispatch::{on_perform, Outcome, PerformCtx},
//!     Domain, EffectLock, KernelConfig, MemoTable, Policy,
//! };
//!
//! # fn main() -> Result<(), Box<dyn core::error::Error>> {
//! let domain = Domain::mint("example.fetch", "url");
//! let request = CanonValue::map([("url", CanonValue::str("https://example.invalid"))]);
//! let encoded = canon::encode(&request)?;
//!
//! let mut table = MemoTable::new();
//! let mut lock = EffectLock::new();
//! let cas = MemoryCas::new();
//! let config = KernelConfig::default();
//!
//! let mut call = |table: &mut MemoTable, lock: &mut EffectLock| {
//!     on_perform(
//!         PerformCtx {
//!             table,
//!             lock,
//!             cas: &cas,
//!             config: &config,
//!             performed_at: "2026-08-02T12:00:00Z",
//!             blessed_by: "example",
//!         },
//!         domain,
//!         &Policy::Keyed,
//!         &encoded,
//!         || Ok::<_, String>(b"body".to_vec()),
//!     )
//! };
//!
//! assert_eq!(call(&mut table, &mut lock)?.outcome, Outcome::Performed);
//! assert_eq!(call(&mut table, &mut lock)?.outcome, Outcome::Hit);
//! # Ok(())
//! # }
//! ```
//!
//! # What is not here yet
//!
//! * The store is a directory or a `BTreeMap`; the prolly-tree store lands
//!   behind the [`Cas`] trait later, which is why that trait is two methods
//!   and no lifetimes.
//! * [`RefreshPolicy::Ttl`] is recorded and not enforced, because enforcing it
//!   needs a clock and the kernel deliberately reads none.
//! * Signatures on pins are carried through the format and never checked.
//! * Nothing calls this from the evaluator yet; effects arrive with the VM.
//!
//! [`Policy::Keyed`]: table::Policy::Keyed
//! [`Policy::Checked`]: table::Policy::Checked
//! [`Policy::Pinned`]: table::Policy::Pinned
//! [`Policy::Transparent`]: table::Policy::Transparent
//! [`Provenance`]: table::Provenance
//! [`Cas`]: cas::Cas
//! [`RefreshPolicy::Ttl`]: table::RefreshPolicy::Ttl

pub mod canon;
pub mod cas;
pub mod dispatch;
pub mod error;
pub mod hash;
pub mod id;
pub mod lock;
pub mod rows;
pub mod table;

pub use canon::{CanonError, CanonValue, DecodeError, decode};
pub use cas::{Cas, DirCas, MemoryCas};
pub use dispatch::{Outcome, PerformCtx, Performed, on_perform};
pub use error::{HashMismatch, KernelError, LockConflict, Result};
pub use hash::Hash;
pub use id::{Domain, Key, ObjId};
pub use lock::{EffectLock, LockRow};
pub use rows::{DirRows, LoadReport, Lookup, Rejected, RowInfo};
pub use table::{Entry, KernelConfig, MemoTable, Policy, Provenance, RefreshPolicy};
