//! The on-disk evaluation cache as one thing, and the sweep that bounds it.
//!
//! Three directories under one root: `objects/` (content-addressed module and
//! result bytes), `index/` (memo rows), `witness/` (the questions each
//! module's evaluation asked). They are separate because they are addressed
//! differently, and they are here together because nothing else knows how all
//! three relate.
//!
//! # Why an unbounded store is not acceptable
//!
//! Measured over an edit loop: a tree of 22 files with one file edited 50
//! times grows by 495 bytes, 3 rows and 1 witness per edit, while the working
//! set stays flat at 22 entries. After 50 edits, 194 rows exist and 22 are
//! ever consulted; the other 89% are versions nobody will ask for again.
//! Growth tracks source size at roughly one copy per edit (a 200 KiB file
//! costs 200 KiB per edit), because a row carries the canonical request and
//! the request carries the source. Editing one large file in a loop is
//! therefore the case that hurts, and it is the ordinary case.
//!
//! # Recency of use, not of write
//!
//! The entries worth keeping under an edit loop are written once and then only
//! read; the churn is freshly written every round. Evicting by write time
//! removes exactly the working set and keeps exactly the garbage, so a hit
//! marks its row used ([`DirRows::touch`], a metadata-only update) and the
//! sweep orders by that.
//!
//! # Eviction can only cause a miss
//!
//! Everything here is [`Policy::Keyed`], so removing any of it is always safe:
//! the effect re-performs. That is what lets the sweep be best-effort, take no
//! lock, and run after the answers have already been given. It is also the
//! gate's assertion: after a sweep, every answer is still byte-identical to a
//! fresh process.
//!
//! [`Policy::Keyed`]: ix_kernel::Policy::Keyed
//! [`DirRows::touch`]: ix_kernel::DirRows::touch

use ix_kernel::Domain;
use ix_kernel::rows::{DirRows, RowInfo};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// What a sweep removed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Witnesses this build could not parse, all of which were removed.
    ///
    /// Reported apart from `witnesses_removed` because the two mean different
    /// things and the difference is the ENG-12601 signature: reclaiming a
    /// witness whose module is genuinely gone is the sweep working, and
    /// finding every witness unreadable is the sweep destroying a cache it
    /// merely failed to understand. A caller that watches only the total
    /// cannot tell those apart, and the total looked healthy while the cache
    /// served nothing.
    pub witnesses_unreadable: usize,
    /// Witnesses still present when the sweep finished, so a caller can see
    /// "removed 5, 0 left" -- which is the shape worth shouting about -- as
    /// distinct from "removed 5, 40 left".
    pub witnesses_left: usize,
    pub bytes_before: u64,
    pub bytes_after: u64,
    pub rows_removed: usize,
    pub objects_removed: usize,
    pub witnesses_removed: usize,
    /// Set when the sweep ran out of things it was willing to remove before
    /// reaching the cap. Worth saying: a cap that cannot be met is a cap
    /// nobody is enforcing, and silence would look like success.
    pub still_over_cap: bool,
}

/// The evaluation cache's directory layout.
#[derive(Clone, Debug)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn open(root: impl Into<PathBuf>) -> std::io::Result<Self> {
        let root = root.into();
        for part in ["objects", "index", "witness"] {
            std::fs::create_dir_all(root.join(part))?;
        }
        Ok(Self { root })
    }

    #[must_use]
    pub fn objects_dir(&self) -> PathBuf {
        self.root.join("objects")
    }

    #[must_use]
    pub fn index_dir(&self) -> PathBuf {
        self.root.join("index")
    }

    #[must_use]
    pub fn witness_dir(&self) -> PathBuf {
        self.root.join("witness")
    }

    /// Total bytes across all three directories.
    #[must_use]
    pub fn size(&self) -> u64 {
        ["objects", "index", "witness"]
            .iter()
            .map(|part| dir_size(&self.root.join(part)))
            .sum()
    }

    /// Remove least-recently-used entries until the store fits in `max_bytes`.
    ///
    /// Rows go first, oldest use first, because they are the largest thing per
    /// entry and the only thing with a usable recency. Then objects nothing
    /// references, then witnesses whose module object is gone. The last two
    /// are not choices: an object no row names can never be found again, and a
    /// witness whose module is absent can only ever produce a replay that
    /// misses.
    ///
    /// `domains` names the row domains to consider. It is a parameter rather
    /// than a scan of `index/` so that a caller cannot accidentally sweep rows
    /// belonging to an effect it does not own.
    pub fn sweep(&self, max_bytes: u64, domains: &[Domain]) -> std::io::Result<SweepReport> {
        let mut report = SweepReport {
            bytes_before: self.size(),
            ..SweepReport::default()
        };
        let mut total = report.bytes_before;

        if total > max_bytes {
            // A row store that will not open means there is nothing to
            // sweep by recency; the object and witness passes below still run.
            let rows = DirRows::open(self.index_dir())
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            let mut inventory: Vec<RowInfo> = Vec::new();
            for domain in domains {
                // A domain that cannot be listed is one this sweep cannot help
                // with; it must not stop the sweep of the others.
                if let Ok(mut found) = rows.inventory(*domain) {
                    inventory.append(&mut found);
                }
            }
            // Oldest use first. Ties broken by name so a sweep is
            // deterministic given the same directory, which is what makes the
            // gate's cap assertion reproducible.
            inventory.sort_by(|left, right| {
                left.used
                    .cmp(&right.used)
                    .then_with(|| left.name.cmp(&right.name))
            });

            // What removing a row actually frees is the row plus, once no
            // surviving row names it, the object it pointed at. Counting only
            // the row understates the saving enormously (a row is a few
            // hundred bytes, its module is kilobytes), and the loop then
            // evicts every row in the store before the object pass reclaims
            // anything. That is not a slow sweep, it is the wrong rows: the
            // recently used ones go too.
            let mut refcount: BTreeMap<String, usize> = BTreeMap::new();
            for row in &inventory {
                if let Some(output) = row.output {
                    *refcount.entry(output.hash().to_hex()).or_default() += 1;
                }
            }
            let object_sizes: BTreeMap<String, u64> = entries(&self.objects_dir())
                .into_iter()
                .map(|(_, bytes, name)| (name, bytes))
                .collect();

            for row in &inventory {
                if total <= max_bytes {
                    break;
                }
                if std::fs::remove_file(&row.path).is_err() {
                    continue;
                }
                total = total.saturating_sub(row.bytes);
                report.rows_removed += 1;
                if let Some(output) = row.output {
                    let name = output.hash().to_hex();
                    let remaining = refcount.entry(name.clone()).or_default();
                    *remaining = remaining.saturating_sub(1);
                    if *remaining == 0 {
                        total = total.saturating_sub(object_sizes.get(&name).copied().unwrap_or(0));
                    }
                }
            }
        }

        // Objects nothing points at, whether or not the cap was reached: an
        // unreferenced object is dead weight regardless of size pressure.
        let referenced = self.referenced_objects(domains);
        for (path, bytes, name) in entries(&self.objects_dir()) {
            if !referenced.contains(&name) && std::fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(bytes);
                report.objects_removed += 1;
            }
        }

        // A witness is dead when the module it names has gone, and it names
        // that module in its own bytes.
        //
        // This used to read the *filename* and look for an object of that
        // name, which worked only while witnesses happened to be named by
        // their module's object address. When they were renamed to the
        // evaluation identity (ENG-12541) no object was ever named that, so
        // every sweep deleted every witness and a capped store served nothing
        // while reporting itself healthy -- arm E of rust-incremental-gate
        // went from 10 hits of 11 to 0 (ENG-12601). Reading the field the
        // writer put there cannot go the same way: a rename is now just a
        // rename.
        //
        // A witness that will not parse is dead too. It can never produce a
        // hit, so keeping it costs bytes and buys nothing.
        for (path, bytes, _) in entries(&self.witness_dir()) {
            let module = std::fs::read(&path)
                .ok()
                .and_then(|raw| crate::readset::witness_module(&raw));
            let alive =
                module.is_some_and(|module| self.objects_dir().join(module.to_hex()).exists());
            if !alive {
                // Which of the two reasons, counted apart. A witness whose
                // module really is gone is ordinary reclamation; one this
                // build cannot parse is a format the store did not expect,
                // and a run where every witness is unreadable is ENG-12601
                // wearing a different hat.
                if module.is_none() {
                    report.witnesses_unreadable += 1;
                }
                if std::fs::remove_file(&path).is_ok() {
                    total = total.saturating_sub(bytes);
                    report.witnesses_removed += 1;
                }
            }
        }

        report.bytes_after = self.size();
        report.still_over_cap = report.bytes_after > max_bytes;
        report.witnesses_left = entries(&self.witness_dir()).len();
        Ok(report)
    }

    /// Hex names of every object some surviving row names.
    fn referenced_objects(&self, domains: &[Domain]) -> BTreeSet<String> {
        let mut referenced = BTreeSet::new();
        let Ok(rows) = DirRows::open(self.index_dir()) else {
            return referenced;
        };
        for domain in domains {
            let Ok(inventory) = rows.inventory(*domain) else {
                continue;
            };
            for row in inventory {
                if let Some(output) = row.output {
                    referenced.insert(output.hash().to_hex());
                }
            }
        }
        referenced
    }
}

/// `(path, bytes, file name)` for every regular file directly in `dir`.
fn entries(dir: &Path) -> Vec<(PathBuf, u64, String)> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    read.filter_map(|entry| {
        let entry = entry.ok()?;
        let name = entry.file_name().to_string_lossy().into_owned();
        // Temporaries belong to a write in flight, not to this sweep.
        if name.starts_with(".tmp-") {
            return None;
        }
        let metadata = entry.metadata().ok()?;
        metadata
            .is_file()
            .then(|| (entry.path(), metadata.len(), name))
    })
    .collect()
}

fn dir_size(dir: &Path) -> u64 {
    let Ok(read) = std::fs::read_dir(dir) else {
        return 0;
    };
    read.filter_map(|entry| {
        let entry = entry.ok()?;
        let metadata = entry.metadata().ok()?;
        if metadata.is_dir() {
            Some(dir_size(&entry.path()))
        } else {
            Some(metadata.len())
        }
    })
    .sum()
}

/// The unreferenced object the sweep would remove: exposed so a scrub can
/// report without deleting.
#[must_use]
pub fn unreferenced_object_names(store: &Store, domains: &[Domain]) -> Vec<String> {
    let referenced = store.referenced_objects(domains);
    entries(&store.objects_dir())
        .into_iter()
        .map(|(_, _, name)| name)
        .filter(|name| !referenced.contains(name))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ix_kernel::Key;
    use ix_kernel::cas::{Cas, DirCas};

    fn scratch(label: &str) -> PathBuf {
        crate::eval::scratch_dir("ixe-store", label)
    }

    fn domain() -> Domain {
        Domain::mint("test.effect", "op")
    }

    /// Write `n` entries, each an object plus a row naming it.
    fn fill(store: &Store, n: usize) -> std::io::Result<Vec<Key>> {
        let cas = DirCas::open(store.objects_dir()).map_err(std::io::Error::other)?;
        let rows = DirRows::open(store.index_dir()).map_err(std::io::Error::other)?;
        let mut keys = Vec::new();
        for i in 0..n {
            let request = format!("request-{i}").into_bytes();
            // Distinct per entry: identical payloads are one object under
            // content addressing, and a single object shared by every row
            // cannot be freed until the last row goes, which is a different
            // situation from the one these tests mean to set up.
            let mut payload = vec![b'p'; 1024];
            payload.extend_from_slice(format!("-{i}").as_bytes());
            let output = cas.put(&payload).map_err(std::io::Error::other)?;
            rows.put(domain(), &request, output)
                .map_err(std::io::Error::other)?;
            keys.push(Key::mint(domain(), &request));
        }
        Ok(keys)
    }

    #[test]
    fn a_store_under_its_cap_is_left_alone() -> std::io::Result<()> {
        let dir = scratch("under");
        let store = Store::open(&dir)?;
        fill(&store, 4)?;
        let before = store.size();
        let report = store.sweep(u64::MAX, &[domain()])?;
        drop(std::fs::remove_dir_all(&dir));
        assert_eq!(report.rows_removed, 0);
        assert_eq!(report.bytes_after, before);
        assert!(!report.still_over_cap);
        Ok(())
    }

    #[test]
    fn sweeping_brings_a_store_under_its_cap() -> std::io::Result<()> {
        let dir = scratch("cap");
        let store = Store::open(&dir)?;
        fill(&store, 20)?;
        let cap = store.size() / 2;
        let report = store.sweep(cap, &[domain()])?;
        let after = store.size();
        drop(std::fs::remove_dir_all(&dir));
        assert!(report.rows_removed > 0, "{report:?}");
        assert!(after <= cap, "{after} bytes with a cap of {cap}");
        assert!(!report.still_over_cap, "{report:?}");
        Ok(())
    }

    /// The property the whole policy exists for. Under an edit loop the
    /// entries worth keeping are the ones being read, not the ones being
    /// written, so a sweep must keep a touched row and drop an untouched one
    /// even though the untouched one was written later.
    #[test]
    fn a_used_row_outlives_a_newer_unused_one() -> std::io::Result<()> {
        let dir = scratch("recency");
        let store = Store::open(&dir)?;
        let rows = DirRows::open(store.index_dir()).map_err(std::io::Error::other)?;
        let keys = fill(&store, 12)?;

        // Age everything, then mark only the first few as used now. Without
        // the explicit ageing this depends on filesystem timestamp
        // granularity, which is coarse enough to make the test lie.
        let old = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        for key in &keys {
            let path = store
                .index_dir()
                .join(domain().hash().to_hex())
                .join(key.hash().to_hex());
            if let Ok(file) = std::fs::File::open(&path) {
                drop(file.set_modified(old));
            }
        }
        let kept: Vec<Key> = keys.iter().take(3).copied().collect();
        for key in &kept {
            assert!(rows.touch(domain(), *key));
        }

        store.sweep(store.size() / 2, &[domain()])?;
        let survived: Vec<bool> = kept
            .iter()
            .map(|key| matches!(rows.get(domain(), *key), ix_kernel::Lookup::Found(_)))
            .collect();
        drop(std::fs::remove_dir_all(&dir));
        assert!(
            survived.iter().all(|&s| s),
            "a touched row was evicted while untouched ones remained: {survived:?}"
        );
        Ok(())
    }

    #[test]
    fn objects_no_row_names_are_reclaimed() -> std::io::Result<()> {
        let dir = scratch("orphans");
        let store = Store::open(&dir)?;
        let cas = DirCas::open(store.objects_dir()).map_err(std::io::Error::other)?;
        fill(&store, 3)?;
        // An object nothing points at.
        let orphan = cas
            .put(b"nobody references this")
            .map_err(std::io::Error::other)?;
        assert!(cas.has(orphan).map_err(std::io::Error::other)?);

        let report = store.sweep(u64::MAX, &[domain()])?;
        let still_there = cas.has(orphan).map_err(std::io::Error::other)?;
        drop(std::fs::remove_dir_all(&dir));
        assert_eq!(report.objects_removed, 1, "{report:?}");
        assert!(!still_there);
        Ok(())
    }

    /// A witness is named by its module's object address, so it is dead
    /// exactly when that object is gone.
    #[test]
    fn witnesses_whose_module_is_gone_are_reclaimed() -> std::io::Result<()> {
        let dir = scratch("witness");
        let store = Store::open(&dir)?;
        fill(&store, 2)?;
        // A real witness, written the way the evaluator writes one, naming a
        // module no object holds. Previously this test wrote the bytes
        // `b"orphan"` under a made-up name, which the sweep discarded whether
        // it judged by filename or by content -- so it passed under a rule
        // that was deleting every witness in the store (ENG-12601). A witness
        // that does not parse proves nothing about how live ones are treated.
        let witness = crate::readset::DirWitness::open(store.witness_dir())?;
        witness.put(&absent_module_identity(), &[])?;

        let report = store.sweep(u64::MAX, &[domain()])?;
        let left = std::fs::read_dir(store.witness_dir())?.count();
        drop(std::fs::remove_dir_all(&dir));
        assert_eq!(report.witnesses_removed, 1, "{report:?}");
        assert_eq!(left, 0);
        Ok(())
    }

    /// The question every identity in these tests is filed under. Which one
    /// does not matter here -- the sweep judges a witness by its module, not
    /// by its question -- so one spelling keeps the tests about the sweep.
    fn whole_question() -> crate::session::Question {
        crate::session::Question::Whole {
            render: crate::session::RenderMode::Plain,
        }
    }

    /// An evaluation identity whose module is not in any store.
    fn absent_module_identity() -> crate::readset::EvalId {
        crate::readset::EvalId::of(
            &ix_kernel::hash::tagged("test-module", &[b"nothing holds this"]),
            &crate::eval::Settings::default(),
            &crate::session::Arguments::none(),
            &whole_question(),
        )
    }

    /// The sweep reports enough to tell housekeeping from destruction.
    ///
    /// ENG-12601 printed "swept 0 rows, 0 objects, 5 witnesses" and read as
    /// tidying up. What made it destruction rather than tidying is that
    /// nothing was left, and the report could not say so: `witnesses_removed`
    /// alone is the same number whether four of forty went or all five of
    /// five. The two extra fields are what let a caller shout, and
    /// `eval-server` does.
    #[test]
    fn a_sweep_that_empties_the_witness_store_says_so() -> std::io::Result<()> {
        let dir = scratch("witness-report");
        let store = Store::open(&dir)?;
        fill(&store, 2)?;
        let witness = crate::readset::DirWitness::open(store.witness_dir())?;
        // Two witnesses whose modules are absent, so both are legitimately
        // reclaimed and nothing is left behind.
        for name in [b"one".as_slice(), b"two".as_slice()] {
            witness.put(
                &crate::readset::EvalId::of(
                    &ix_kernel::hash::tagged("test-module", &[name]),
                    &crate::eval::Settings::default(),
                    &crate::session::Arguments::none(),
                    &whole_question(),
                ),
                &[],
            )?;
        }
        // And one the store cannot parse at all, which is the ENG-12601
        // signature rather than ordinary reclamation.
        std::fs::write(store.witness_dir().join("a".repeat(64)), b"not canon")?;

        let report = store.sweep(u64::MAX, &[domain()])?;
        drop(std::fs::remove_dir_all(&dir));

        assert_eq!(report.witnesses_removed, 3, "{report:?}");
        assert_eq!(report.witnesses_left, 0, "{report:?}");
        assert_eq!(
            report.witnesses_unreadable, 1,
            "an unparseable witness must be counted apart from a reclaimed one, \
             because a run where every witness is unreadable is a broken format \
             and not a tidy store: {report:?}"
        );
        Ok(())
    }

    /// The assertion whose absence let ENG-12601 through: a witness whose
    /// module is still in the store must survive a sweep.
    ///
    /// Every witness test here was a removal test, so a rule that removed
    /// *everything* satisfied all of them. That is what shipped: the sweep
    /// judged a witness by its filename, the filename stopped being the
    /// module's object address, and every sweep emptied the witness
    /// directory. A capped store then served nothing while reporting itself
    /// under cap and healthy -- arm E of rust-incremental-gate went from 10
    /// hits of 11 to 0, and nothing else noticed.
    #[test]
    fn a_witness_whose_module_is_present_survives_a_sweep() -> std::io::Result<()> {
        let dir = scratch("witness-live");
        let store = Store::open(&dir)?;
        let cas = DirCas::open(store.objects_dir()).map_err(std::io::Error::other)?;
        fill(&store, 2)?;

        // A module object really in the store, and a row naming it, because
        // that is the shape the evaluator produces: `ModuleCache` writes a
        // compile-domain row for every module it stores, and the server
        // sweeps `[compile_domain(), eval_domain()]` so the object is
        // reachable. Without the row the object is unreferenced, the object
        // pass reclaims it first, and the witness is then dead for a reason
        // that has nothing to do with what this test is about.
        let module_bytes = b"a compiled module the witness belongs to";
        let module = cas.put(module_bytes).map_err(std::io::Error::other)?;
        let rows = DirRows::open(store.index_dir()).map_err(std::io::Error::other)?;
        rows.put(domain(), b"the-module-row", module)
            .map_err(std::io::Error::other)?;
        let identity = crate::readset::EvalId::of(
            module.hash(),
            &crate::eval::Settings::default(),
            &crate::session::Arguments::none(),
            &whole_question(),
        );
        let witness = crate::readset::DirWitness::open(store.witness_dir())?;
        witness.put(&identity, &[])?;

        let report = store.sweep(u64::MAX, &[domain()])?;
        let survived = witness.get(&identity).is_some();
        let left = std::fs::read_dir(store.witness_dir())?.count();
        drop(std::fs::remove_dir_all(&dir));

        assert_eq!(
            report.witnesses_removed, 0,
            "the sweep reclaimed a witness whose module is still here: {report:?}"
        );
        assert_eq!(left, 1);
        assert_eq!(
            report.objects_removed, 0,
            "the module object was reclaimed, so this test is not exercising \
             the live-witness case: {report:?}"
        );
        assert!(
            survived,
            "the witness file is gone, so every later process starts cold"
        );
        Ok(())
    }

    /// A cap smaller than anything the sweep is willing to remove has to be
    /// reported, not silently missed: a cap nobody enforces is worse than no
    /// cap, because it reads as enforced.
    #[test]
    fn an_unreachable_cap_is_reported() -> std::io::Result<()> {
        let dir = scratch("unreachable");
        let store = Store::open(&dir)?;
        fill(&store, 2)?;
        let report = store.sweep(1, &[domain()])?;
        drop(std::fs::remove_dir_all(&dir));
        // Everything removable went, and the store still cannot fit in 1 byte
        // only if something remains; either way the flag must match reality.
        assert_eq!(report.still_over_cap, report.bytes_after > 1);
        Ok(())
    }

    /// Rows belonging to a domain the caller did not name are not swept, so a
    /// sweep cannot reclaim another effect's cache.
    #[test]
    fn rows_of_unnamed_domains_are_untouched() -> std::io::Result<()> {
        let dir = scratch("scoped");
        let store = Store::open(&dir)?;
        fill(&store, 10)?;
        let other = Domain::mint("test.effect", "other-op");
        let report = store.sweep(1, &[other])?;
        let rows_left =
            std::fs::read_dir(store.index_dir().join(domain().hash().to_hex()))?.count();
        drop(std::fs::remove_dir_all(&dir));
        assert_eq!(report.rows_removed, 0);
        assert_eq!(rows_left, 10, "{report:?}");
        Ok(())
    }

    #[test]
    fn sweeping_an_empty_store_is_a_no_op() -> std::io::Result<()> {
        let dir = scratch("empty");
        let store = Store::open(&dir)?;
        let report = store.sweep(0, &[domain()])?;
        drop(std::fs::remove_dir_all(&dir));
        assert_eq!(report, SweepReport::default());
        Ok(())
    }
}
