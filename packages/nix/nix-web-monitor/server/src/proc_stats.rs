//! Per-build cpu/memory/generation sampling for the machine-wide view.
//!
//! `nix store builds --json` reports each goal's builder pid but nothing about
//! what that process is doing to the machine. This module fills the gap
//! without any nix change: between polls the global probe samples procfs for
//! every reported pid's *process subtree* (builders fork compilers, which fork
//! more) and annotates each goal with cpu percent (delta of
//! `utime+stime+cutime+cstime` from `/proc/<pid>/stat` over wall time) and
//! resident memory (`VmRSS` from `/proc/<pid>/status`). The waited-child
//! fields matter: compilers that fork and exit between two polls would
//! otherwise vanish from the live subtree sum and under-report the build.
//!
//! The cpu/rss columns are Linux-only, but the third annotation -- the
//! worker's kernel start time, i.e. its *generation* -- must not be: it is
//! what tells a pid recycled for the same derivation within the same
//! whole-second `startTime` apart from its predecessor, so without it an open
//! log drawer could silently retarget (see `start_ticks` on `GlobalBuild`).
//! The status payload itself carries no per-worker field (`logFile` is a pure
//! function of the drv path), so on a host without procfs (macOS/nix-darwin)
//! the generation comes from `sysctl(KERN_PROC_PID)` instead.
//!
//! Best-effort like the rest of the global view: a pid that exits mid-sample
//! or an unreadable `/proc` entry just leaves the fields `None` and the UI
//! omits the columns. Nothing here can fail the probe.

use std::collections::HashMap;
use std::time::Instant;

use nix_web_monitor_parser::GlobalBuild;

/// Kernel ticks per second for `/proc/<pid>/stat` cpu fields. Linux fixes the
/// userspace-visible `USER_HZ` at 100 regardless of the kernel's internal HZ,
/// so a constant avoids a libc dependency for `sysconf(_SC_CLK_TCK)`.
const TICKS_PER_SECOND: f64 = 100.0;

/// One process as read from `/proc/<pid>/stat`: enough to build the process
/// tree and integrate cpu time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ProcessIdentity {
    pid: i64,
    start_ticks: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ProcStat {
    identity: ProcessIdentity,
    ppid: i64,
    /// `utime + stime + cutime + cstime` in clock ticks: the process's own
    /// cpu plus that of exited children it has already waited for, so
    /// short-lived compilers reaped between polls still count.
    cpu_ticks: u64,
}

#[derive(Clone, Copy, Debug)]
struct CpuBaseline {
    cpu_ticks: u64,
    sampled_at: Instant,
}

/// What a CPU baseline belongs to: the process *and* the goal it was building
/// when sampled. The process identity alone is not enough -- a long-lived
/// nix-daemon worker keeps its `(pid, start_ticks)` while finishing one
/// derivation and picking up the next, so a baseline keyed only by process
/// would hand the new build's first sample the previous build's ticks. The
/// goal fields (drv or store path plus the goal's start second, all straight
/// from the `nix store builds` payload) separate sequential builds on one
/// worker; a poll where any of them changed starts the counter fresh.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct BaselineKey {
    process: ProcessIdentity,
    drv_path: Option<String>,
    store_path: Option<String>,
    start_time: Option<i64>,
}

impl BaselineKey {
    fn for_build(build: &GlobalBuild, process: ProcessIdentity) -> Self {
        Self {
            process,
            drv_path: build.drv_path.clone(),
            store_path: build.store_path.clone(),
            start_time: build.start_time,
        }
    }
}

/// Stateful sampler: remembers each build pid's subtree cpu ticks from the
/// previous poll so the next poll can turn the delta into a rate.
#[derive(Debug, Default)]
pub struct BuildStatSampler {
    previous: HashMap<BaselineKey, CpuBaseline>,
}

impl BuildStatSampler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Annotate `builds` with sampled cpu/rss for every goal with a pid.
    /// First sight of a pid records a baseline and reports rss only; the next
    /// poll (two seconds later) starts reporting cpu.
    pub fn annotate(&mut self, builds: &mut [GlobalBuild]) {
        if !builds.iter().any(|build| build.pid.is_some()) {
            self.previous.clear();
            return;
        }
        self.annotate_from(builds, &read_proc_table(), start_generation_without_procfs);
    }

    /// [`annotate`](Self::annotate) against an explicit proc table and
    /// no-procfs generation source, so tests can exercise both platforms'
    /// wiring on either.
    fn annotate_from(
        &mut self,
        builds: &mut [GlobalBuild],
        table: &HashMap<i64, ProcStat>,
        fallback_generation: impl Fn(i64) -> Option<u64>,
    ) {
        let now = Instant::now();
        let children = children_by_parent(table);
        let mut next = HashMap::new();

        for build in builds.iter_mut() {
            let Some(pid) = build.pid else { continue };
            let Some(root) = table.get(&pid) else {
                // No procfs row: the builder is already gone, or the host has
                // no procfs at all (macOS/nix-darwin). cpu/rss genuinely need
                // the table and stay absent, but the generation must not go
                // with them -- `None` here is what lets a same-second pid
                // recycle silently retarget an open log drawer -- so it falls
                // back to the platform's non-procfs start-time source.
                build.start_ticks = fallback_generation(pid);
                continue;
            };
            let key = BaselineKey::for_build(build, root.identity);
            let subtree = subtree_pids(pid, &children);
            let ticks: u64 = subtree
                .iter()
                .filter_map(|p| table.get(p))
                .map(|stat| stat.cpu_ticks)
                .sum();
            let rss = aggregate_rss(&subtree, read_rss_bytes);

            build.rss_bytes = rss;
            build.cpu_percent = self.cpu_percent_for(&key, ticks, now);
            // The worker's kernel start ticks are its generation: the payload's
            // `start_time` is whole seconds, so this is what tells a recycled
            // pid apart from its predecessor within the same second. The UI
            // keys log drawers on it and `/api/global-log` matches it exactly.
            build.start_ticks = Some(root.identity.start_ticks);
            next.insert(
                key,
                CpuBaseline {
                    cpu_ticks: ticks,
                    sampled_at: now,
                },
            );
        }
        // Keep baselines only for goals still active, so neither a recycled
        // pid nor a worker's next build inherits a stale counter.
        self.previous = next;
    }

    fn cpu_percent_for(&self, key: &BaselineKey, cpu_ticks: u64, now: Instant) -> Option<u32> {
        let baseline = self.previous.get(key)?;
        let elapsed = now.duration_since(baseline.sampled_at).as_secs_f64();
        if elapsed <= 0.0 {
            return None;
        }
        Some(cpu_percent(
            cpu_ticks.saturating_sub(baseline.cpu_ticks),
            elapsed,
        ))
    }
}

/// The worker's start generation on a host without procfs: macOS/nix-darwin,
/// where it is the kernel's per-process start timestamp in microseconds from
/// `sysctl(KERN_PROC_PID)` -- unprivileged for any pid (the same interface
/// `ps` uses), so it covers the daemon's root-owned workers too. Not
/// comparable to Linux's ticks-since-boot, and it does not need to be: a
/// generation is an opaque value the UI echoes back verbatim and
/// `/api/global-log` matches exactly. `None` when the process is already
/// gone, mirroring the procfs path.
#[cfg(target_os = "macos")]
fn start_generation_without_procfs(pid: i64) -> Option<u64> {
    // Prefix of XNU's `struct kinfo_proc` (bsd/sys/sysctl.h): its first field
    // is `kp_proc` (`struct extern_proc`, bsd/sys/proc.h), whose first field
    // is the union `p_un` -- run-queue pointers in the live kernel, but
    // `struct timeval __p_starttime` in the copied-out view: the kernel's
    // `fill_user64_externproc` (bsd/kern/kern_sysctl.c) writes the start time
    // into exactly that member (`#define p_starttime p_un.__p_starttime`),
    // which is how `ps` computes `lstart`. So the start `timeval` sits at
    // offset 0, and the kernel's `user64_timeval` ({i64 seconds, i32 micros})
    // matches darwin's native `timeval` field layout. Only that field is
    // typed here; the rest is opaque, over-sized padding so the kernel's
    // copyout (648 bytes on 64-bit) always fits.
    #[repr(C)]
    struct KinfoProcPrefix {
        start_time: libc::timeval,
        _rest: [u8; 1024],
    }

    let pid = libc::pid_t::try_from(pid).ok()?;
    let mut mib = [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_PID, pid];
    let mut info = KinfoProcPrefix {
        start_time: libc::timeval { tv_sec: 0, tv_usec: 0 },
        _rest: [0; 1024],
    };
    let mut size = std::mem::size_of::<KinfoProcPrefix>();
    // SAFETY: `mib` and the output buffer are live locals sized as passed;
    // the kernel writes at most `size` bytes and updates `size` to what it
    // actually wrote. No pointer outlives the call.
    let status = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            4,
            (&raw mut info).cast(),
            &raw mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    // A pid that is already gone reports success with nothing written, so
    // require the start `timeval` to actually be there.
    if status != 0 || size < std::mem::size_of::<libc::timeval>() {
        return None;
    }
    let seconds = u64::try_from(info.start_time.tv_sec).ok()?;
    let micros = u64::try_from(info.start_time.tv_usec).ok()?;
    Some(seconds * 1_000_000 + micros)
}

/// On Linux the proc table is the (richer) generation source, so a missing
/// row only ever means the process is gone: there is nothing to fall back to.
#[cfg(not(target_os = "macos"))]
const fn start_generation_without_procfs(_pid: i64) -> Option<u64> {
    None
}

/// Sum the readable resident sizes in a process subtree. `None` distinguishes
/// an unreadable/restricted procfs snapshot from a measured zero-byte total.
fn aggregate_rss(
    subtree: &[i64],
    mut read_rss: impl FnMut(i64) -> Option<u64>,
) -> Option<u64> {
    subtree
        .iter()
        .filter_map(|pid| read_rss(*pid))
        .reduce(|total, rss| total + rss)
}

/// Ticks spent over wall seconds, as whole percent of one core.
#[allow(
    clippy::cast_precision_loss, // tick counts are far below 2^52
    clippy::cast_possible_truncation, // percent of a machine's cores fits u32
    clippy::cast_sign_loss // ticks and elapsed are non-negative
)]
fn cpu_percent(delta_ticks: u64, elapsed_seconds: f64) -> u32 {
    ((delta_ticks as f64 / TICKS_PER_SECOND / elapsed_seconds) * 100.0).round() as u32
}

/// Every numeric `/proc` entry's stat line, keyed by pid. Empty on platforms
/// without procfs.
fn read_proc_table() -> HashMap<i64, ProcStat> {
    let mut table = HashMap::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return table;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|n| n.parse::<i64>().ok()) else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        if let Some(parsed) = parse_stat_line(pid, &stat) {
            table.insert(pid, parsed);
        }
    }
    table
}

/// Parse one `/proc/<pid>/stat` line into identity, parent, and cpu ticks.
///
/// The second field is `(comm)` and may itself contain spaces and parentheses
/// (`(tokio-runtime-w)`, even `(a) b)`), so split on the *last* `)` before
/// counting fields. After the comm, 1-indexed field 3 is the state, 4 the
/// ppid, 14/15 utime/stime, 16/17 cutime/cstime -- i.e. rest[1], rest[11],
/// rest[12], rest[13], rest[14]. Field 22 is the process start time in kernel
/// ticks, or rest[19], which distinguishes a reused pid from its predecessor.
/// The child fields are signed in proc(5), so they parse as `i64` and clamp at
/// zero.
fn parse_stat_line(pid: i64, line: &str) -> Option<ProcStat> {
    let rest = line.rsplit_once(')')?.1;
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let parent = fields.get(1)?.parse::<i64>().ok()?;
    let utime = fields.get(11)?.parse::<u64>().ok()?;
    let stime = fields.get(12)?.parse::<u64>().ok()?;
    let child_user = fields.get(13)?.parse::<i64>().ok()?;
    let child_system = fields.get(14)?.parse::<i64>().ok()?;
    let start_ticks = fields.get(19)?.parse::<u64>().ok()?;
    // Clamp-then-convert is infallible: `max(0)` makes `unsigned_abs` the
    // identity, so a (never observed in practice) negative field counts as 0.
    let child_ticks = child_user.max(0).unsigned_abs() + child_system.max(0).unsigned_abs();
    Some(ProcStat {
        identity: ProcessIdentity { pid, start_ticks },
        ppid: parent,
        cpu_ticks: utime + stime + child_ticks,
    })
}

/// `VmRSS` from `/proc/<pid>/status`, in bytes. `None` when the process is
/// gone, unreadable, or a kernel thread with no memory rows.
fn read_rss_bytes(pid: i64) -> Option<u64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    parse_vmrss_bytes(&status)
}

/// Extract `VmRSS:    1234 kB` from a `/proc/<pid>/status` blob. The kernel
/// always reports kB.
fn parse_vmrss_bytes(status: &str) -> Option<u64> {
    let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
    let kilobytes: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kilobytes * 1024)
}

/// Children per ppid, for subtree walks.
fn children_by_parent(table: &HashMap<i64, ProcStat>) -> HashMap<i64, Vec<i64>> {
    let mut children: HashMap<i64, Vec<i64>> = HashMap::new();
    for stat in table.values() {
        children
            .entry(stat.ppid)
            .or_default()
            .push(stat.identity.pid);
    }
    children
}

/// The pid plus every transitive child, cycle-safe (procfs snapshots taken
/// dir-entry by dir-entry can be momentarily inconsistent).
fn subtree_pids(root: i64, children: &HashMap<i64, Vec<i64>>) -> Vec<i64> {
    let mut seen = vec![root];
    let mut queue = vec![root];
    while let Some(pid) = queue.pop() {
        for &child in children.get(&pid).into_iter().flatten() {
            if !seen.contains(&child) {
                seen.push(child);
                queue.push(child);
            }
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A comm containing spaces and a `)` must not shift the field offsets;
    /// splitting on the last `)` keeps ppid/utime/stime aligned.
    #[test]
    fn stat_line_parses_despite_hostile_comm() {
        // pid (comm) state ppid pgrp session tty tpgid flags minflt cminflt
        // majflt cmajflt utime stime cutime cstime ...
        let line = "4242 (a) b (c)) R 100 4242 4242 0 -1 4194304 1 0 0 0 700 42 5 3 20 0 1 0 9000";
        let stat = parse_stat_line(4242, line).expect("hostile comm parses");
        assert_eq!(
            stat,
            ProcStat {
                identity: ProcessIdentity {
                    pid: 4242,
                    start_ticks: 9000,
                },
                ppid: 100,
                // utime + stime + cutime + cstime: waited-for children count.
                cpu_ticks: 750
            }
        );
    }

    #[test]
    fn stat_line_rejects_garbage() {
        assert_eq!(parse_stat_line(1, "not a stat line"), None);
        assert_eq!(parse_stat_line(1, "1 (x) R 2"), None);
    }

    #[test]
    fn vmrss_parses_and_scales_to_bytes() {
        let status = "Name:\tcc1plus\nVmPeak:\t  999 kB\nVmRSS:\t    2048 kB\nThreads:\t1\n";
        assert_eq!(parse_vmrss_bytes(status), Some(2048 * 1024));
        // Kernel threads have no Vm* rows at all.
        assert_eq!(parse_vmrss_bytes("Name:\tkworker\n"), None);
    }

    /// Subtree aggregation follows fork chains and survives a (snapshot-skew)
    /// parent cycle without looping.
    #[test]
    fn subtree_covers_descendants_and_tolerates_cycles() {
        fn stat(pid: i64, parent: i64, cpu_ticks: u64) -> ProcStat {
            ProcStat {
                identity: ProcessIdentity {
                    pid,
                    start_ticks: pid.unsigned_abs(),
                },
                ppid: parent,
                cpu_ticks,
            }
        }
        let table: HashMap<i64, ProcStat> = [
            (10, stat(10, 1, 5)),
            (11, stat(11, 10, 7)),
            (12, stat(12, 11, 9)),
            // Skewed snapshot: 13 claims 12 as parent, 12's ancestor chain
            // reaches 13 -> cycle in the child map.
            (13, stat(13, 12, 1)),
            (20, stat(20, 1, 100)),
        ]
        .into_iter()
        .collect();
        let mut children = children_by_parent(&table);
        children.entry(13).or_default().push(11);

        let mut subtree = subtree_pids(10, &children);
        subtree.sort_unstable();
        assert_eq!(subtree, vec![10, 11, 12, 13]);
    }

    /// A baseline key for tests: `process` plus the goal fields of `build`.
    fn key_for(build: &GlobalBuild, pid: i64, start_ticks: u64) -> BaselineKey {
        BaselineKey::for_build(build, ProcessIdentity { pid, start_ticks })
    }

    #[test]
    fn recycled_pid_does_not_inherit_cpu_baseline() {
        let sampled_at = Instant::now();
        let build = GlobalBuild {
            drv_path: Some("/nix/store/aaa-hello.drv".to_owned()),
            start_time: Some(1_700_000_000),
            ..GlobalBuild::default()
        };
        let original = key_for(&build, 42, 100);
        let recycled = key_for(&build, 42, 200);
        let sampler = BuildStatSampler {
            previous: HashMap::from([(
                original.clone(),
                CpuBaseline {
                    cpu_ticks: 1000,
                    sampled_at,
                },
            )]),
        };
        let one_second_later = sampled_at + std::time::Duration::from_secs(1);

        assert_eq!(
            sampler.cpu_percent_for(&original, 1050, one_second_later),
            Some(50)
        );
        assert_eq!(
            sampler.cpu_percent_for(&recycled, 1050, one_second_later),
            None
        );
    }

    /// A long-lived daemon worker that finishes one derivation and starts
    /// another between polls keeps its process identity; the goal fields in
    /// the key must stop the new build's first sample from inheriting the old
    /// build's ticks.
    #[test]
    fn next_goal_on_same_worker_starts_without_baseline() {
        let sampled_at = Instant::now();
        let first = GlobalBuild {
            drv_path: Some("/nix/store/aaa-first.drv".to_owned()),
            start_time: Some(1_700_000_000),
            ..GlobalBuild::default()
        };
        let second = GlobalBuild {
            drv_path: Some("/nix/store/bbb-second.drv".to_owned()),
            start_time: Some(1_700_000_060),
            ..GlobalBuild::default()
        };
        let finished = key_for(&first, 42, 100);
        let started = key_for(&second, 42, 100);
        let sampler = BuildStatSampler {
            previous: HashMap::from([(
                finished,
                CpuBaseline {
                    cpu_ticks: 1000,
                    sampled_at,
                },
            )]),
        };
        let one_second_later = sampled_at + std::time::Duration::from_secs(1);

        assert_eq!(
            sampler.cpu_percent_for(&started, 1050, one_second_later),
            None
        );
    }

    #[test]
    fn idle_sample_clears_baselines_without_procfs_work() {
        let mut sampler = BuildStatSampler {
            previous: HashMap::from([(
                key_for(&GlobalBuild::default(), 42, 100),
                CpuBaseline {
                    cpu_ticks: 1000,
                    sampled_at: Instant::now(),
                },
            )]),
        };
        let mut pidless = vec![GlobalBuild::default()];

        sampler.annotate(&mut pidless);

        assert!(sampler.previous.is_empty());
    }

    #[test]
    fn rss_is_absent_when_every_status_read_fails() {
        let subtree = [10, 11, 12];
        assert_eq!(aggregate_rss(&subtree, |_| None), None);
        assert_eq!(
            aggregate_rss(&subtree, |pid| (pid != 11).then_some(pid.unsigned_abs())),
            Some(22)
        );
    }

    /// One own-pid build annotated through the public entry point, shared by
    /// the platform end-to-end tests below. The sampler keeps its baselines
    /// across calls (they key on process+goal identity, not list identity),
    /// so calling this twice models two polls of the same build.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn annotate_own_process(sampler: &mut BuildStatSampler) -> GlobalBuild {
        let mut builds = vec![GlobalBuild {
            pid: Some(i64::from(std::process::id())),
            ..GlobalBuild::default()
        }];
        sampler.annotate(&mut builds);
        builds.remove(0)
    }

    /// End-to-end on the live procfs: sampling our own pid twice yields a
    /// resident size and (second sample) a cpu figure.
    #[cfg(target_os = "linux")]
    #[test]
    fn sampler_annotates_own_process() {
        let mut sampler = BuildStatSampler::new();
        let first = annotate_own_process(&mut sampler);
        assert!(first.rss_bytes.is_some_and(|rss| rss > 0));
        // The worker generation is annotated from the very first sample.
        assert!(first.start_ticks.is_some());
        // First sample has no baseline yet.
        assert_eq!(first.cpu_percent, None);

        std::thread::sleep(std::time::Duration::from_millis(30));
        assert!(annotate_own_process(&mut sampler).cpu_percent.is_some());
    }

    /// Without a procfs row for the pid -- the builder exited mid-poll, or
    /// the host has no procfs at all (macOS/nix-darwin) -- the worker
    /// generation comes from the platform fallback, so the same-second
    /// pid-recycle hole stays closed off-Linux too. cpu/rss genuinely need
    /// the proc table and stay absent, and no cpu baseline is kept.
    #[test]
    fn missing_proc_row_takes_fallback_generation() {
        // (goal pid, fallback's answer for that pid) -> expected start_ticks.
        // The fallback asserts it is only ever asked for the goal's own pid,
        // so the pidless row doubles as "no process, no lookup".
        let cases: [(Option<i64>, Option<u64>, Option<u64>); 3] = [
            (Some(42), Some(1_720_200_000_123_456), Some(1_720_200_000_123_456)),
            (Some(43), None, None),
            (None, Some(777), None),
        ];
        for (pid, generation, expected) in cases {
            let mut sampler = BuildStatSampler::new();
            let mut builds = vec![GlobalBuild {
                pid,
                ..GlobalBuild::default()
            }];
            sampler.annotate_from(&mut builds, &HashMap::new(), |asked| {
                assert_eq!(Some(asked), pid, "fallback asked for the goal's pid only");
                generation
            });
            assert_eq!(builds[0].start_ticks, expected, "generation for pid {pid:?}");
            assert_eq!(builds[0].cpu_percent, None);
            assert_eq!(builds[0].rss_bytes, None);
            assert!(sampler.previous.is_empty(), "no baseline without a proc row");
        }
    }

    /// End-to-end on the live sysctl source: sampling our own pid yields a
    /// generation even though the host has no procfs, and a plausible one
    /// (a start time after 2020, not in the future).
    #[cfg(target_os = "macos")]
    #[test]
    fn sampler_annotates_own_process_generation_without_procfs() {
        let mut sampler = BuildStatSampler::new();
        let build = annotate_own_process(&mut sampler);

        let generation = build.start_ticks.expect("own process has a generation");
        let now_micros = u64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock is past the epoch")
                .as_micros(),
        )
        .expect("microseconds since epoch fit u64");
        assert!(generation > 1_577_836_800_000_000, "start is after 2020");
        assert!(generation <= now_micros, "start is not in the future");
        // No procfs: the Linux-only columns stay absent.
        assert_eq!(build.cpu_percent, None);
        assert_eq!(build.rss_bytes, None);
    }

    /// A pid that no longer exists yields no stats and no stale baseline.
    #[test]
    fn dead_pid_is_left_unannotated() {
        let mut sampler = BuildStatSampler::new();
        let mut builds = vec![GlobalBuild {
            // Above any real pid_max.
            pid: Some(9_999_999_999),
            ..GlobalBuild::default()
        }];
        sampler.annotate(&mut builds);
        assert_eq!(builds[0].cpu_percent, None);
        assert_eq!(builds[0].rss_bytes, None);
        assert_eq!(builds[0].start_ticks, None);
        assert!(sampler.previous.is_empty());
    }
}
