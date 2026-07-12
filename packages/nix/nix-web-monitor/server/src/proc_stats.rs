//! Per-build cpu/memory sampling for the machine-wide view (Linux only).
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
//! Best-effort like the rest of the global view: a pid that exits mid-sample,
//! an unreadable `/proc` entry, or a non-Linux host just leaves the fields
//! `None` and the UI omits the columns. Nothing here can fail the probe.

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

/// Stateful sampler: remembers each build pid's subtree cpu ticks from the
/// previous poll so the next poll can turn the delta into a rate.
#[derive(Debug, Default)]
pub struct BuildStatSampler {
    previous: HashMap<ProcessIdentity, CpuBaseline>,
}

impl BuildStatSampler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Annotate `builds` with sampled cpu/rss for every goal with a pid.
    /// First sight of a pid records a baseline and reports rss only; the next
    /// poll (two seconds later) starts reporting cpu.
    pub fn annotate(&mut self, builds: &mut [GlobalBuild]) {
        let now = Instant::now();
        let table = read_proc_table();
        let children = children_by_parent(&table);
        let mut next = HashMap::new();

        for build in builds.iter_mut() {
            let Some(pid) = build.pid else { continue };
            let Some(root) = table.get(&pid) else {
                // Builder already gone (or no procfs): nothing to report, and
                // drop any stale baseline.
                continue;
            };
            let identity = root.identity;
            let subtree = subtree_pids(pid, &children);
            let ticks: u64 = subtree
                .iter()
                .filter_map(|p| table.get(p))
                .map(|stat| stat.cpu_ticks)
                .sum();
            let rss: u64 = subtree.iter().filter_map(|p| read_rss_bytes(*p)).sum();

            build.rss_bytes = Some(rss);
            build.cpu_percent = self.cpu_percent_for(identity, ticks, now);
            next.insert(
                identity,
                CpuBaseline {
                    cpu_ticks: ticks,
                    sampled_at: now,
                },
            );
        }
        // Keep baselines only for pids still active, so a recycled pid never
        // inherits a stale counter.
        self.previous = next;
    }

    fn cpu_percent_for(
        &self,
        identity: ProcessIdentity,
        cpu_ticks: u64,
        now: Instant,
    ) -> Option<u32> {
        let baseline = self.previous.get(&identity)?;
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

    #[test]
    fn recycled_pid_does_not_inherit_cpu_baseline() {
        let sampled_at = Instant::now();
        let original = ProcessIdentity {
            pid: 42,
            start_ticks: 100,
        };
        let recycled = ProcessIdentity {
            pid: 42,
            start_ticks: 200,
        };
        let sampler = BuildStatSampler {
            previous: HashMap::from([(
                original,
                CpuBaseline {
                    cpu_ticks: 1000,
                    sampled_at,
                },
            )]),
        };
        let one_second_later = sampled_at + std::time::Duration::from_secs(1);

        assert_eq!(
            sampler.cpu_percent_for(original, 1050, one_second_later),
            Some(50)
        );
        assert_eq!(
            sampler.cpu_percent_for(recycled, 1050, one_second_later),
            None
        );
    }

    /// End-to-end on the live procfs: sampling our own pid twice yields a
    /// resident size and (second sample) a cpu figure.
    #[cfg(target_os = "linux")]
    #[test]
    fn sampler_annotates_own_process() {
        let mut sampler = BuildStatSampler::new();
        let mut builds = vec![GlobalBuild {
            pid: Some(i64::from(std::process::id())),
            ..GlobalBuild::default()
        }];
        sampler.annotate(&mut builds);
        assert!(builds[0].rss_bytes.is_some_and(|rss| rss > 0));
        // First sample has no baseline yet.
        assert_eq!(builds[0].cpu_percent, None);

        std::thread::sleep(std::time::Duration::from_millis(30));
        sampler.annotate(&mut builds);
        assert!(builds[0].cpu_percent.is_some());
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
        assert!(sampler.previous.is_empty());
    }
}
