// Copyright 2026 The Jujutsu Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Per-procedure counts and in-handler time, so a slow mount can be diagnosed
//! rather than guessed at.
//!
//! A wall clock number on its own does not say whether a filesystem is slow
//! because each operation is expensive, because there are too many operations,
//! or because the work inside each one is expensive. Those have three different
//! fixes and only one of them is "use a faster transport". This records the two
//! numbers that separate them: how many of each procedure arrived, and how long
//! was spent inside our handler for it. Wall clock minus the sum of the second
//! is everything that is not us, which for a loopback RPC server is dominated
//! by round trips.
//!
//! Deliberately counters and not a histogram. A mean plus a count answers "is
//! this latency or volume", which is the question, and a histogram would need a
//! dependency and a decision about bucket layout to answer a question nobody
//! has asked yet.

use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Instant;

/// The NFSv3 procedures this server implements, plus the overlay operations
/// worth separating out because they are the ones that can be slow for reasons
/// of our own rather than the protocol's.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Op {
    /// Resolve a name in a directory.
    Lookup,
    /// Fetch attributes.
    Getattr,
    /// Set attributes, including truncate and chmod.
    Setattr,
    /// Read file content.
    Read,
    /// Write file content.
    Write,
    /// Create a regular file.
    Create,
    /// Create a directory.
    Mkdir,
    /// Unlink a file or remove a directory.
    Remove,
    /// Rename.
    Rename,
    /// Create a symlink.
    Symlink,
    /// Read a symlink target.
    Readlink,
    /// List a directory.
    Readdir,
    /// Flush, which for this server has nothing to do.
    Commit,
    /// Copy a tracked file into the writable layer. Not a procedure: it happens
    /// inside a write or a setattr, and it reads a blob out of the store, so it
    /// is the one part of a write whose cost no transport change would touch.
    CopyUp,
    /// A write to an AppleDouble sidecar, accepted and discarded. Counted
    /// separately because the interesting number is how much of the traffic is
    /// the transport talking to itself.
    Sidecar,
    /// Hide a tracked name the caller deleted. Counted because it is the one
    /// operation that makes the mount show less than its revision, so "how
    /// many" is the first thing to ask when a file is unexpectedly missing.
    Whiteout,
}

impl Op {
    /// Every variant, for reporting. Kept beside the enum so a new procedure
    /// that is not added here shows up as a missing row rather than silently
    /// vanishing from the report.
    pub const ALL: [Self; 16] = [
        Self::Lookup,
        Self::Getattr,
        Self::Setattr,
        Self::Read,
        Self::Write,
        Self::Create,
        Self::Mkdir,
        Self::Remove,
        Self::Rename,
        Self::Symlink,
        Self::Readlink,
        Self::Readdir,
        Self::Commit,
        Self::CopyUp,
        Self::Sidecar,
        Self::Whiteout,
    ];

    /// Name used in the report.
    pub fn name(self) -> &'static str {
        match self {
            Self::Lookup => "lookup",
            Self::Getattr => "getattr",
            Self::Setattr => "setattr",
            Self::Read => "read",
            Self::Write => "write",
            Self::Create => "create",
            Self::Mkdir => "mkdir",
            Self::Remove => "remove",
            Self::Rename => "rename",
            Self::Symlink => "symlink",
            Self::Readlink => "readlink",
            Self::Readdir => "readdir",
            Self::Commit => "commit",
            Self::CopyUp => "copy-up",
            Self::Sidecar => "sidecar-discard",
            Self::Whiteout => "whiteout",
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Lookup => 0,
            Self::Getattr => 1,
            Self::Setattr => 2,
            Self::Read => 3,
            Self::Write => 4,
            Self::Create => 5,
            Self::Mkdir => 6,
            Self::Remove => 7,
            Self::Rename => 8,
            Self::Symlink => 9,
            Self::Readlink => 10,
            Self::Readdir => 11,
            Self::Commit => 12,
            Self::CopyUp => 13,
            Self::Sidecar => 14,
            Self::Whiteout => 15,
        }
    }
}

/// One procedure's totals.
#[derive(Clone, Copy, Debug)]
pub struct OpStats {
    /// Which procedure.
    pub op: Op,
    /// How many arrived.
    pub count: u64,
    /// Total time spent inside our handler.
    pub nanos: u64,
    /// Bytes moved, for the two procedures where that is meaningful.
    pub bytes: u64,
}

impl OpStats {
    /// Mean time inside our handler, in microseconds.
    pub fn mean_micros(&self) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        // f64 rather than integer division: the interesting values here are
        // single-digit microseconds and integer division reports those as 0,
        // which reads as "free" rather than "small".
        self.nanos as f64 / self.count as f64 / 1000.0
    }
}

/// Counters for one served mount.
#[derive(Debug, Default)]
pub struct Stats {
    counts: [AtomicU64; 16],
    nanos: [AtomicU64; 16],
    bytes: [AtomicU64; 16],
}

impl Stats {
    /// A fresh set of counters.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one completed operation.
    ///
    /// `Relaxed` throughout. These are diagnostics, nothing branches on them,
    /// and a stronger ordering would put a barrier in the hot path of the thing
    /// being measured, which is its own kind of wrong answer.
    pub fn record(&self, op: Op, elapsed_nanos: u64, bytes: u64) {
        let index = op.index();
        self.counts[index].fetch_add(1, Ordering::Relaxed);
        self.nanos[index].fetch_add(elapsed_nanos, Ordering::Relaxed);
        self.bytes[index].fetch_add(bytes, Ordering::Relaxed);
    }

    /// A snapshot of every procedure's totals.
    pub fn snapshot(&self) -> Vec<OpStats> {
        Op::ALL
            .into_iter()
            .map(|op| {
                let index = op.index();
                OpStats {
                    op,
                    count: self.counts[index].load(Ordering::Relaxed),
                    nanos: self.nanos[index].load(Ordering::Relaxed),
                    bytes: self.bytes[index].load(Ordering::Relaxed),
                }
            })
            .collect()
    }

    /// Total operations and total time inside our handlers.
    pub fn totals(&self) -> (u64, u64) {
        let snapshot = self.snapshot();
        let count = snapshot.iter().map(|stats| stats.count).sum();
        let nanos = snapshot.iter().map(|stats| stats.nanos).sum();
        (count, nanos)
    }

    /// A one-line-per-procedure report, omitting procedures that never ran.
    ///
    /// The last line is the one that answers the question: total operations,
    /// total time we spent, and therefore how much of any wall clock
    /// measurement is not us.
    pub fn report(&self) -> String {
        use std::fmt::Write as _;

        let mut out = String::new();
        let mut total_count = 0;
        let mut total_nanos = 0;
        out.push_str("procedure         count      mean us     total s\n");
        for stats in self.snapshot() {
            total_count += stats.count;
            total_nanos += stats.nanos;
            if stats.count == 0 {
                continue;
            }
            let _ = writeln!(
                out,
                "{:<16} {:>7}   {:>10.1}   {:>9.3}",
                stats.op.name(),
                stats.count,
                stats.mean_micros(),
                stats.nanos as f64 / 1e9,
            );
        }
        let _ = writeln!(
            out,
            "{:<16} {:>7}   {:>10.1}   {:>9.3}",
            "TOTAL",
            total_count,
            if total_count == 0 {
                0.0
            } else {
                total_nanos as f64 / total_count as f64 / 1000.0
            },
            total_nanos as f64 / 1e9,
        );
        out
    }
}

/// Times one operation and records it on drop.
///
/// A guard rather than a closure so that the `?` operator still works inside
/// the thing being measured; wrapping a fallible body in a closure to time it
/// is how timing code ends up changing the code it times.
pub struct Timer<'a> {
    stats: &'a Stats,
    op: Op,
    started: Instant,
    bytes: u64,
}

impl<'a> Timer<'a> {
    /// Starts timing `op`.
    pub fn new(stats: &'a Stats, op: Op) -> Self {
        Self {
            stats,
            op,
            started: Instant::now(),
            bytes: 0,
        }
    }

    /// Records a byte count alongside the timing, for read and write.
    pub fn bytes(&mut self, bytes: u64) {
        self.bytes = bytes;
    }
}

impl Drop for Timer<'_> {
    fn drop(&mut self) {
        let elapsed = u64::try_from(self.started.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.stats.record(self.op, elapsed, self.bytes);
    }
}
