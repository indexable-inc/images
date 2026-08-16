//! The M0 gate, minus the VM.
//!
//! M0 asks for a program that performs effects to run twice and do no work the
//! second time, with each discipline behaving as advertised. There is no
//! evaluator to drive the kernel yet, so the program here is written by hand:
//! four effects, one per discipline, performed through [`on_perform`] with
//! closures that count how many times they ran.
//!
//! The invocation count is the whole point. Every other signal -- the output
//! bytes, the object address, the exit status -- is identical whether the
//! second run hit the table or re-performed and happened to agree, so a test
//! that checks only the answer cannot tell a working cache from a cache that
//! does nothing. Counting is the only assertion that separates them.

use ix_kernel::{
    Domain, EffectLock, KernelConfig, KernelError, MemoTable, ObjId, Policy, Provenance,
    RefreshPolicy, Result,
    canon::{self, CanonValue},
    cas::{Cas, DirCas, MemoryCas},
    dispatch::{Outcome, PerformCtx, Performed, on_perform},
};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// Fixed so that a recorded pin is the same bytes on every run. A kernel that
/// read a clock could not have this test.
const WHEN: &str = "2026-08-02T12:00:00Z";
const WHO: &str = "m0-gate";

/// How many times each named effect actually ran.
#[derive(Debug, Default)]
struct Ledger {
    calls: RefCell<BTreeMap<&'static str, u32>>,
}

impl Ledger {
    fn perform(&self, name: &'static str, output: &[u8]) -> core::result::Result<Vec<u8>, String> {
        *self.calls.borrow_mut().entry(name).or_default() += 1;
        Ok(output.to_vec())
    }

    fn count(&self, name: &str) -> u32 {
        self.calls.borrow().get(name).copied().unwrap_or_default()
    }

    fn total(&self) -> u32 {
        self.calls.borrow().values().sum()
    }
}

/// The kernel a program runs against: one table, one store, one lock.
struct Kernel<C: Cas> {
    table: MemoTable,
    lock: EffectLock,
    cas: C,
    config: KernelConfig,
}

impl<C: Cas> Kernel<C> {
    fn new(cas: C) -> Self {
        Self {
            table: MemoTable::new(),
            lock: EffectLock::new(),
            cas,
            config: KernelConfig::default(),
        }
    }

    fn perform<F>(
        &mut self,
        domain: Domain,
        policy: &Policy,
        req: &[u8],
        run: F,
    ) -> Result<Performed>
    where
        F: FnOnce() -> core::result::Result<Vec<u8>, String>,
    {
        on_perform(
            PerformCtx {
                table: &mut self.table,
                lock: &mut self.lock,
                cas: &self.cas,
                config: &self.config,
                performed_at: WHEN,
                blessed_by: WHO,
            },
            domain,
            policy,
            req,
            run,
        )
    }
}

fn domain(op: &str) -> Domain {
    Domain::mint("m0.gate", op)
}

/// Canonically encoded request. Real callers build these from their own
/// request types; the gate uses one shape for all four effects.
fn request(name: &str) -> Vec<u8> {
    // Cannot fail: a one-entry map of strings has no float, no oversized
    // integer and no duplicate key. Defaulting to empty rather than
    // unwrapping keeps the test binary panic-free; an empty encoding would
    // collide every request onto one key, which every count assertion below
    // would notice immediately.
    canon::encode(&CanonValue::map([("name", CanonValue::str(name))])).unwrap_or_default()
}

/// Outputs the program's effects produce, so declarations can be written
/// against them.
const READ_OUTPUT: &[u8] = b"contents of the checked-in file";
const FETCH_OUTPUT: &[u8] = b"whatever the network said today";
const TARBALL_OUTPUT: &[u8] = b"tarball bytes";

/// The program: read a file (`Keyed`), fetch a tarball against a declared hash
/// (`Checked`), fetch a mutable URL (`Pinned`), and log (`Transparent`).
fn run_program<C: Cas>(kernel: &mut Kernel<C>, ledger: &Ledger) -> Result<Vec<Outcome>> {
    let read = kernel.perform(
        domain("read"),
        &Policy::Keyed,
        &request("build.conf"),
        || ledger.perform("read", READ_OUTPUT),
    )?;

    let tarball = kernel.perform(
        domain("tarball"),
        &Policy::Checked {
            declared: *ObjId::of(TARBALL_OUTPUT).hash(),
        },
        &request("source.tar.gz"),
        || ledger.perform("tarball", TARBALL_OUTPUT),
    )?;

    let fetch = kernel.perform(
        domain("fetch"),
        &Policy::Pinned(RefreshPolicy::Manual),
        &request("https://example.invalid/latest"),
        || ledger.perform("fetch", FETCH_OUTPUT),
    )?;

    let log = kernel.perform(
        domain("log"),
        &Policy::Transparent,
        &request("starting build"),
        || ledger.perform("log", b""),
    )?;

    Ok(vec![
        read.outcome,
        tarball.outcome,
        fetch.outcome,
        log.outcome,
    ])
}

/// The gate itself: run the program twice and check that the second run does
/// no work beyond the effect that is supposed to always run.
#[test]
fn the_second_run_performs_only_the_transparent_effect() -> Result<()> {
    let ledger = Ledger::default();
    let mut kernel = Kernel::new(MemoryCas::new());

    let first = run_program(&mut kernel, &ledger)?;
    assert_eq!(
        first,
        vec![
            Outcome::Performed,
            Outcome::Performed,
            Outcome::Performed,
            Outcome::Transparent,
        ],
        "everything is a miss on a cold kernel"
    );
    assert_eq!(ledger.total(), 4);

    let second = run_program(&mut kernel, &ledger)?;
    assert_eq!(
        second,
        vec![
            Outcome::Hit,
            Outcome::Hit,
            Outcome::Hit,
            Outcome::Transparent,
        ],
        "the memoised three hit, the transparent one runs again"
    );

    assert_eq!(ledger.count("read"), 1, "keyed performed once");
    assert_eq!(ledger.count("tarball"), 1, "checked performed once");
    assert_eq!(ledger.count("fetch"), 1, "pinned performed once");
    assert_eq!(ledger.count("log"), 2, "transparent performed every time");
    Ok(())
}

#[test]
fn each_discipline_records_the_provenance_it_promises() -> Result<()> {
    let ledger = Ledger::default();
    let mut kernel = Kernel::new(MemoryCas::new());
    run_program(&mut kernel, &ledger)?;

    let provenance = |op: &str, name: &str| {
        let domain = domain(op);
        kernel
            .table
            .get(domain, ix_kernel::Key::mint(domain, &request(name)))
            .map(|entry| entry.provenance.clone())
    };

    assert_eq!(
        provenance("read", "build.conf"),
        Some(Provenance::Deterministic)
    );
    assert_eq!(
        provenance("tarball", "source.tar.gz"),
        Some(Provenance::Verified {
            declared: *ObjId::of(TARBALL_OUTPUT).hash(),
        })
    );
    assert_eq!(
        provenance("fetch", "https://example.invalid/latest"),
        Some(Provenance::Blessed {
            who: WHO.to_owned(),
            when: WHEN.to_owned(),
            sig: None,
        })
    );
    // Transparent effects leave no row at all, so there is nothing to vouch for.
    assert_eq!(provenance("log", "starting build"), None);
    Ok(())
}

#[test]
fn a_wrong_declaration_hard_fails_and_records_nothing() {
    let ledger = Ledger::default();
    let mut kernel = Kernel::new(MemoryCas::new());

    let refused = kernel.perform(
        domain("tarball"),
        &Policy::Checked {
            // The hash of some other release: the sort of mistake a bad rebase
            // or a copied line produces.
            declared: *ObjId::of(b"the previous release").hash(),
        },
        &request("source.tar.gz"),
        || ledger.perform("tarball", TARBALL_OUTPUT),
    );

    assert!(
        matches!(refused, Err(KernelError::HashMismatch(_))),
        "expected a hard failure, got {refused:?}"
    );
    if let Err(KernelError::HashMismatch(detail)) = &refused {
        assert_eq!(detail.actual, ObjId::of(TARBALL_OUTPUT));
        assert_eq!(detail.declared, *ObjId::of(b"the previous release").hash());
    }
    assert_eq!(ledger.count("tarball"), 1, "performed once, then refused");
    assert!(
        kernel.table.is_empty(),
        "a rejected output must not be memoised, or a retry would serve it"
    );
}

#[test]
fn a_frozen_kernel_with_no_pins_refuses_and_names_the_row() {
    let ledger = Ledger::default();
    let mut kernel = Kernel::new(MemoryCas::new());
    kernel.config.frozen = true;

    let refused = kernel.perform(
        domain("fetch"),
        &Policy::Pinned(RefreshPolicy::Manual),
        &request("https://example.invalid/latest"),
        || ledger.perform("fetch", FETCH_OUTPUT),
    );

    assert!(
        matches!(refused, Err(KernelError::FrozenPin { .. })),
        "expected a frozen refusal, got {refused:?}"
    );
    assert_eq!(ledger.count("fetch"), 0, "a frozen miss never performs");

    // The message has to name the row, because the next thing anyone does is
    // go and look for it in effect.lock.
    let expected_domain = domain("fetch").to_string();
    let expected_key =
        ix_kernel::Key::mint(domain("fetch"), &request("https://example.invalid/latest"))
            .to_string();
    let message = refused.map_or_else(|error| error.to_string(), |_| String::new());
    assert!(message.contains(&expected_domain), "message: {message}");
    assert!(message.contains(&expected_key), "message: {message}");
}

#[test]
fn transparent_effects_leave_the_store_empty() -> Result<()> {
    let ledger = Ledger::default();
    let mut kernel = Kernel::new(MemoryCas::new());
    for _ in 0..3 {
        let performed = kernel.perform(
            domain("log"),
            &Policy::Transparent,
            &request("starting build"),
            || ledger.perform("log", b""),
        )?;
        assert_eq!(performed.outcome, Outcome::Transparent);
    }
    assert_eq!(ledger.count("log"), 3);
    assert!(kernel.table.is_empty(), "no rows");
    assert!(kernel.cas.is_empty(), "no objects");
    assert!(kernel.lock.is_empty(), "no pins");
    Ok(())
}

/// Save, reload, save again: the bytes must not move. Anything else turns a
/// lock file into a source of diff noise, and a diff nobody reads is a review
/// nobody does.
#[test]
fn the_lock_file_survives_a_round_trip_byte_for_byte() -> Result<()> {
    let ledger = Ledger::default();
    let mut kernel = Kernel::new(MemoryCas::new());
    run_program(&mut kernel, &ledger)?;
    // Two pins, so ordering between rows is actually exercised.
    kernel.perform(
        domain("fetch"),
        &Policy::Pinned(RefreshPolicy::Manual),
        &request("https://example.invalid/other"),
        || ledger.perform("fetch2", b"other"),
    )?;

    let workspace = scratch("lock-round-trip")?;
    let path = workspace.join("effect.lock");
    kernel.lock.save(&path)?;
    let first = read(&path)?;

    let reloaded = EffectLock::load(&path)?;
    reloaded.save(&path)?;
    let second = read(&path)?;

    let cleanup = std::fs::remove_dir_all(&workspace);
    assert_eq!(first, second, "saving reloaded pins moved the bytes");
    assert_eq!(reloaded, kernel.lock, "reloading lost or changed a pin");
    assert_eq!(reloaded.len(), 2, "both pins are in the file");
    // The pins are legible: a reviewer sees who, when, and what.
    assert!(first.contains(WHO), "the file names who pinned:\n{first}");
    assert!(first.contains(WHEN), "the file names when:\n{first}");
    drop(cleanup);
    Ok(())
}

/// The replay path, which is what makes a frozen build possible: a brand new
/// process with an empty table and a store it did not fill still serves the
/// pinned answer, without performing.
#[test]
fn a_fresh_process_replays_pins_without_performing() -> Result<()> {
    let workspace = scratch("replay")?;
    let store = workspace.join("store");
    let path = workspace.join("effect.lock");

    // First process: cold, unfrozen, performs and records.
    let ledger = Ledger::default();
    let mut first = Kernel::new(DirCas::open(&store)?);
    run_program(&mut first, &ledger)?;
    first.lock.save(&path)?;
    assert_eq!(ledger.count("fetch"), 1);

    // Second process: nothing in memory, frozen, and only the lock file plus
    // the on-disk store to go on.
    let mut second = Kernel::new(DirCas::open(&store)?);
    second.config.frozen = true;
    EffectLock::load(&path)?.replay_into(&mut second.table);

    let pinned = second.perform(
        domain("fetch"),
        &Policy::Pinned(RefreshPolicy::Manual),
        &request("https://example.invalid/latest"),
        || ledger.perform("fetch", b"a different answer today"),
    )?;

    let bytes = second.cas.get(pinned.output)?;
    let cleanup = std::fs::remove_dir_all(&workspace);

    assert_eq!(pinned.outcome, Outcome::Hit);
    assert_eq!(ledger.count("fetch"), 1, "replay must not re-perform");
    assert_eq!(
        bytes,
        Some(FETCH_OUTPUT.to_vec()),
        "the pinned bytes come back, not today's answer"
    );
    drop(cleanup);
    Ok(())
}

/// Replay only covers what is in the file. An effect the lock does not mention
/// still fails under a frozen kernel, which is what stops a locked build from
/// picking up a new dependency by accident.
#[test]
fn replay_does_not_cover_effects_the_lock_never_saw() -> Result<()> {
    let ledger = Ledger::default();
    let mut recorded = Kernel::new(MemoryCas::new());
    run_program(&mut recorded, &ledger)?;

    let mut frozen = Kernel::new(MemoryCas::new());
    frozen.config.frozen = true;
    recorded.lock.replay_into(&mut frozen.table);

    let refused = frozen.perform(
        domain("fetch"),
        &Policy::Pinned(RefreshPolicy::Manual),
        &request("https://example.invalid/newly-added"),
        || ledger.perform("fetch-new", b"new"),
    );
    assert!(
        matches!(refused, Err(KernelError::FrozenPin { .. })),
        "expected a refusal for the unpinned request, got {refused:?}"
    );
    assert_eq!(ledger.count("fetch-new"), 0);
    Ok(())
}

static SCRATCH: AtomicU64 = AtomicU64::new(0);

fn scratch(label: &str) -> Result<PathBuf> {
    let path = std::env::temp_dir().join(format!(
        "ix-kernel-m0-{label}-{}-{}",
        std::process::id(),
        SCRATCH.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path)
        .map_err(|source| io_error(format!("creating {}", path.display()), source))?;
    Ok(path)
}

fn read(path: &std::path::Path) -> Result<String> {
    std::fs::read_to_string(path)
        .map_err(|source| io_error(format!("reading {}", path.display()), source))
}

/// The crate's `io` constructor is crate-private, so tests build the variant
/// directly rather than widening the API for a test's convenience.
fn io_error(doing: String, source: std::io::Error) -> KernelError {
    KernelError::Io { doing, source }
}
