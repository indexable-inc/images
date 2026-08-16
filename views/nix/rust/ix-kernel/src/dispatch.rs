//! Discipline dispatch: the one place an effect gets performed.
//!
//! Every effect goes through [`on_perform`], and its [`Policy`] decides
//! whether the memo table is consulted, whether the answer is checked, whether
//! a miss is allowed to invent a new pin, and what provenance the row ends up
//! carrying. Keeping the four disciplines in one `match` is the point: it is
//! the only place to look to find out what the kernel is willing to trust, and
//! adding a fifth discipline cannot be done by accident somewhere else.

use crate::cas::Cas;
use crate::error::{KernelError, Result};
use crate::id::{Domain, Key, ObjId};
use crate::lock::{EffectLock, LockRow};
use crate::table::{Entry, KernelConfig, MemoTable, Policy, Provenance};
use core::fmt;

/// Everything a perform touches, plus the two facts the kernel refuses to
/// find out for itself.
///
/// `performed_at` and `blessed_by` are injected rather than read here. A
/// kernel that calls a clock is a kernel whose output depends on when it ran,
/// which makes the same build unreproducible and every test that touches a pin
/// non-deterministic. The caller owns the clock; the kernel owns the rows.
///
/// The fields are bundled into a struct rather than passed as seven arguments
/// because they travel together and always will.
pub struct PerformCtx<'a, C: Cas + ?Sized> {
    pub table: &'a mut MemoTable,
    /// Pins recorded by `Pinned` effects land here. Callers with nothing to
    /// pin can pass a scratch [`EffectLock`] and ignore it.
    pub lock: &'a mut EffectLock,
    pub cas: &'a C,
    pub config: &'a KernelConfig,
    /// RFC 3339 timestamp written into any pin this call records.
    pub performed_at: &'a str,
    /// Who a newly recorded pin is attributed to.
    pub blessed_by: &'a str,
}

/// What a call did, for callers that want to report or meter it. The [`ObjId`]
/// alone cannot distinguish a hit from a miss, and "did this build perform
/// anything" is exactly the question a frozen CI run wants to ask.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Served from the memo table; the effect did not run.
    Hit,
    /// The effect ran and the row was recorded.
    Performed,
    /// The effect ran and nothing was recorded (`Transparent` only).
    Transparent,
}

/// The result of a dispatched effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Performed {
    pub output: ObjId,
    pub key: Key,
    pub outcome: Outcome,
}

/// Perform an effect under its discipline.
///
/// `req_canon` is the canonically encoded request, not the request: encoding
/// is the caller's job because only the caller knows the request's type, and
/// the kernel must not be generic over it. [`crate::canon::encode`] produces
/// the bytes this expects.
///
/// The four disciplines:
///
/// * `Keyed` -- look up, and on a miss perform and record. Re-performing is
///   safe by assumption, so nothing needs to be verified or vouched for.
/// * `Checked` -- perform at most once, and the output must hash to the
///   declared value or the call fails. The declaration is re-checked on a hit
///   too, because it is not part of the key: without that, editing the
///   expected hash would keep serving the output taken under the old one, and
///   a declaration nobody rechecks is a comment.
/// * `Pinned` -- trust on first use. Under [`KernelConfig::frozen`] a miss is
///   refused, naming the domain and key, so a locked build cannot quietly
///   acquire a new dependency.
/// * `Transparent` -- always perform, never record, and the output must be
///   empty. See [`Policy::Transparent`].
pub fn on_perform<C, E, F>(
    ctx: PerformCtx<'_, C>,
    domain: Domain,
    policy: &Policy,
    req_canon: &[u8],
    perform: F,
) -> Result<Performed>
where
    C: Cas + ?Sized,
    E: fmt::Display,
    F: FnOnce() -> core::result::Result<Vec<u8>, E>,
{
    let key = Key::mint(domain, req_canon);
    let run = |perform: F| -> Result<Vec<u8>> {
        perform().map_err(|error| KernelError::Perform {
            domain,
            detail: error.to_string(),
        })
    };

    match policy {
        Policy::Transparent => {
            let output = run(perform)?;
            if !output.is_empty() {
                return Err(KernelError::TransparentNotUnit {
                    domain,
                    len: output.len(),
                });
            }
            // Nothing is stored, so the address is the one every empty output
            // shares. It is returned for uniformity, not to be looked up.
            Ok(Performed {
                output: ObjId::of(&[]),
                key,
                outcome: Outcome::Transparent,
            })
        }

        Policy::Keyed => {
            if let Some(entry) = ctx.table.get(domain, key) {
                return Ok(hit(entry.output, key));
            }
            let output = ctx.cas.put(&run(perform)?)?;
            ctx.table.insert(
                domain,
                key,
                Entry {
                    output,
                    policy: policy.clone(),
                    provenance: Provenance::Deterministic,
                },
            );
            Ok(performed(output, key))
        }

        Policy::Checked { declared } => {
            if let Some(entry) = ctx.table.get(domain, key) {
                let stored = entry.output;
                return if stored.hash() == declared {
                    Ok(hit(stored, key))
                } else {
                    Err(KernelError::mismatch(domain, key, *declared, stored))
                };
            }
            let output = ctx.cas.put(&run(perform)?)?;
            if output.hash() != declared {
                // Hard error, and the row is not recorded: a mismatch means
                // either the declaration or the effect is wrong, and both want
                // a human before anything downstream sees the bytes.
                return Err(KernelError::mismatch(domain, key, *declared, output));
            }
            ctx.table.insert(
                domain,
                key,
                Entry {
                    output,
                    policy: policy.clone(),
                    provenance: Provenance::Verified {
                        declared: *declared,
                    },
                },
            );
            Ok(performed(output, key))
        }

        Policy::Pinned(_) => {
            if let Some(entry) = ctx.table.get(domain, key) {
                return Ok(hit(entry.output, key));
            }
            if ctx.config.frozen {
                return Err(KernelError::FrozenPin { domain, key });
            }
            let output = ctx.cas.put(&run(perform)?)?;
            let provenance = Provenance::Blessed {
                who: ctx.blessed_by.to_owned(),
                when: ctx.performed_at.to_owned(),
                sig: None,
            };
            ctx.table.insert(
                domain,
                key,
                Entry {
                    output,
                    policy: policy.clone(),
                    provenance,
                },
            );
            // The row and the pin are written together. A pin recorded in the
            // table but not the file would evaporate at process exit and be
            // re-taken, silently, as a different answer.
            ctx.lock.insert(
                domain,
                LockRow {
                    label: None,
                    key,
                    output,
                    canon_version: crate::canon::VERSION.to_owned(),
                    performed_at: ctx.performed_at.to_owned(),
                    by: ctx.blessed_by.to_owned(),
                    sig: None,
                },
            );
            Ok(performed(output, key))
        }
    }
}

const fn hit(output: ObjId, key: Key) -> Performed {
    Performed {
        output,
        key,
        outcome: Outcome::Hit,
    }
}

const fn performed(output: ObjId, key: Key) -> Performed {
    Performed {
        output,
        key,
        outcome: Outcome::Performed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cas::MemoryCas;
    use crate::table::RefreshPolicy;
    use std::cell::Cell;

    /// A perform closure that counts how many times it ran. The count is the
    /// only way to tell a hit from a re-perform that happened to agree.
    struct Effect {
        calls: Cell<u32>,
        output: Vec<u8>,
    }

    impl Effect {
        fn new(output: &[u8]) -> Self {
            Self {
                calls: Cell::new(0),
                output: output.to_vec(),
            }
        }

        fn run(&self) -> core::result::Result<Vec<u8>, String> {
            self.calls.set(self.calls.get() + 1);
            Ok(self.output.clone())
        }
    }

    struct Fixture {
        table: MemoTable,
        lock: EffectLock,
        cas: MemoryCas,
        config: KernelConfig,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                table: MemoTable::new(),
                lock: EffectLock::new(),
                cas: MemoryCas::new(),
                config: KernelConfig::default(),
            }
        }

        fn call<E: fmt::Display, F: FnOnce() -> core::result::Result<Vec<u8>, E>>(
            &mut self,
            policy: &Policy,
            perform: F,
        ) -> Result<Performed> {
            on_perform(
                PerformCtx {
                    table: &mut self.table,
                    lock: &mut self.lock,
                    cas: &self.cas,
                    config: &self.config,
                    performed_at: "2026-08-02T12:00:00Z",
                    blessed_by: "tester",
                },
                domain(),
                policy,
                b"req",
                perform,
            )
        }
    }

    fn domain() -> Domain {
        Domain::mint("test-effect", "op")
    }

    #[test]
    fn keyed_performs_once_then_hits() -> Result<()> {
        let mut fixture = Fixture::new();
        let effect = Effect::new(b"answer");
        let first = fixture.call(&Policy::Keyed, || effect.run())?;
        let second = fixture.call(&Policy::Keyed, || effect.run())?;
        assert_eq!(effect.calls.get(), 1);
        assert_eq!(first.outcome, Outcome::Performed);
        assert_eq!(second.outcome, Outcome::Hit);
        assert_eq!(first.output, second.output);
        assert_eq!(fixture.cas.get(first.output)?, Some(b"answer".to_vec()));
        Ok(())
    }

    #[test]
    fn keyed_rows_are_deterministic_provenance() -> Result<()> {
        let mut fixture = Fixture::new();
        let effect = Effect::new(b"answer");
        let performed = fixture.call(&Policy::Keyed, || effect.run())?;
        let entry = fixture.table.get(domain(), performed.key);
        assert_eq!(
            entry.map(|entry| &entry.provenance),
            Some(&Provenance::Deterministic)
        );
        Ok(())
    }

    #[test]
    fn checked_accepts_a_matching_declaration() -> Result<()> {
        let mut fixture = Fixture::new();
        let effect = Effect::new(b"answer");
        let policy = Policy::Checked {
            declared: *ObjId::of(b"answer").hash(),
        };
        let first = fixture.call(&policy, || effect.run())?;
        let second = fixture.call(&policy, || effect.run())?;
        assert_eq!(effect.calls.get(), 1, "checked performs at most once");
        assert_eq!(second.outcome, Outcome::Hit);
        assert_eq!(first.output, ObjId::of(b"answer"));
        Ok(())
    }

    #[test]
    fn checked_hard_fails_on_a_wrong_declaration() {
        let mut fixture = Fixture::new();
        let effect = Effect::new(b"answer");
        let policy = Policy::Checked {
            declared: *ObjId::of(b"something else").hash(),
        };
        let refused = fixture.call(&policy, || effect.run());
        assert!(
            matches!(refused, Err(KernelError::HashMismatch(_))),
            "expected a mismatch, got {refused:?}"
        );
        assert_eq!(effect.calls.get(), 1, "performed exactly once");
        // Nothing is recorded, so a retry re-performs rather than serving the
        // rejected bytes from the table.
        assert!(fixture.table.is_empty());
    }

    /// The declaration is not part of the key, so a hit has to be rechecked.
    /// Changing the declared hash must not keep serving the old output.
    #[test]
    fn checked_rechecks_the_declaration_on_a_hit() -> Result<()> {
        let mut fixture = Fixture::new();
        let effect = Effect::new(b"answer");
        fixture.call(
            &Policy::Checked {
                declared: *ObjId::of(b"answer").hash(),
            },
            || effect.run(),
        )?;
        let refused = fixture.call(
            &Policy::Checked {
                declared: *ObjId::of(b"revised").hash(),
            },
            || effect.run(),
        );
        assert!(
            matches!(refused, Err(KernelError::HashMismatch(_))),
            "expected a mismatch on the revised declaration, got {refused:?}"
        );
        assert_eq!(effect.calls.get(), 1, "the effect is not re-run to check");
        Ok(())
    }

    #[test]
    fn pinned_takes_one_answer_and_records_it() -> Result<()> {
        let mut fixture = Fixture::new();
        let effect = Effect::new(b"whatever-came-back");
        let policy = Policy::Pinned(RefreshPolicy::Manual);
        let first = fixture.call(&policy, || effect.run())?;
        let second = fixture.call(&policy, || effect.run())?;
        assert_eq!(effect.calls.get(), 1);
        assert_eq!(second.outcome, Outcome::Hit);
        let pin = fixture.lock.get(domain(), first.key);
        assert_eq!(pin.map(|row| row.output), Some(first.output));
        assert_eq!(pin.map(|row| row.by.as_str()), Some("tester"));
        assert_eq!(
            pin.map(|row| row.performed_at.as_str()),
            Some("2026-08-02T12:00:00Z")
        );
        Ok(())
    }

    #[test]
    fn a_frozen_kernel_refuses_to_mint_a_pin() {
        let mut fixture = Fixture::new();
        fixture.config.frozen = true;
        let effect = Effect::new(b"x");
        let refused = fixture.call(&Policy::Pinned(RefreshPolicy::Manual), || effect.run());
        assert!(
            matches!(refused, Err(KernelError::FrozenPin { .. })),
            "expected a frozen refusal, got {refused:?}"
        );
        assert_eq!(effect.calls.get(), 0, "a frozen miss never performs");
        assert!(fixture.lock.is_empty());
    }

    #[test]
    fn a_frozen_kernel_still_serves_a_replayed_pin() -> Result<()> {
        let mut fixture = Fixture::new();
        let effect = Effect::new(b"pinned");
        let policy = Policy::Pinned(RefreshPolicy::Manual);
        fixture.call(&policy, || effect.run())?;

        // A fresh process: the table is gone, the lock file is not.
        let mut replayed = Fixture::new();
        replayed.config.frozen = true;
        fixture.lock.replay_into(&mut replayed.table);
        let served = replayed.call(&policy, || effect.run())?;
        assert_eq!(served.outcome, Outcome::Hit);
        assert_eq!(effect.calls.get(), 1);
        Ok(())
    }

    #[test]
    fn transparent_always_performs_and_stores_nothing() -> Result<()> {
        let mut fixture = Fixture::new();
        let effect = Effect::new(b"");
        let first = fixture.call(&Policy::Transparent, || effect.run())?;
        let second = fixture.call(&Policy::Transparent, || effect.run())?;
        assert_eq!(effect.calls.get(), 2);
        assert_eq!(first.outcome, Outcome::Transparent);
        assert_eq!(second.outcome, Outcome::Transparent);
        assert!(fixture.table.is_empty());
        assert!(fixture.cas.is_empty());
        assert!(fixture.lock.is_empty());
        Ok(())
    }

    #[test]
    fn transparent_refuses_a_value() {
        let mut fixture = Fixture::new();
        let effect = Effect::new(b"not unit");
        let refused = fixture.call(&Policy::Transparent, || effect.run());
        assert!(
            matches!(refused, Err(KernelError::TransparentNotUnit { len: 8, .. })),
            "expected a unit refusal, got {refused:?}"
        );
    }

    #[test]
    fn a_failing_effect_records_nothing() {
        let mut fixture = Fixture::new();
        let refused = fixture.call(&Policy::Keyed, || {
            core::result::Result::<Vec<u8>, String>::Err("upstream is down".to_owned())
        });
        assert!(
            matches!(refused, Err(KernelError::Perform { .. })),
            "expected the effect's failure, got {refused:?}"
        );
        // The effect's own message survives to the caller.
        assert!(
            refused
                .map_or_else(|error| error.to_string(), |_| String::new())
                .contains("upstream is down")
        );
        assert!(fixture.table.is_empty());
        assert!(fixture.cas.is_empty());
    }

    #[test]
    fn different_requests_do_not_share_a_row() -> Result<()> {
        let mut fixture = Fixture::new();
        let effect = Effect::new(b"answer");
        let first = on_perform(
            PerformCtx {
                table: &mut fixture.table,
                lock: &mut fixture.lock,
                cas: &fixture.cas,
                config: &fixture.config,
                performed_at: "t",
                blessed_by: "tester",
            },
            domain(),
            &Policy::Keyed,
            b"one",
            || effect.run(),
        )?;
        let second = on_perform(
            PerformCtx {
                table: &mut fixture.table,
                lock: &mut fixture.lock,
                cas: &fixture.cas,
                config: &fixture.config,
                performed_at: "t",
                blessed_by: "tester",
            },
            domain(),
            &Policy::Keyed,
            b"two",
            || effect.run(),
        )?;
        assert_ne!(first.key, second.key);
        assert_eq!(effect.calls.get(), 2);
        Ok(())
    }
}
