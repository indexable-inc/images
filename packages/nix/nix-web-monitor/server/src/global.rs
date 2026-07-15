//! Machine-wide build probe (best-effort, patched-nix only).
//!
//! Polls a patched-nix subcommand, `nix store builds --json`, which reads a
//! daemon-independent status directory and prints every active build/substitution
//! goal on the host, with the why-chain (root derivation -> ... -> this goal)
//! that scheduled it. The rest of the monitor only ever sees one invocation's
//! tree; this is the one view of *everything* the machine is building right now,
//! and why.
//!
//! The subcommand exists only on a patched nix, so the probe auto-detects: it
//! runs the command and, if the output does not parse as a JSON build array
//! (stock nix prints an "unknown command" error instead), marks the view
//! undetected and the UI hides the panel. It never returns and never panics:
//! every failure becomes a status string, and the
//! loop backs off and retries so a mid-session nix upgrade is eventually picked
//! up.
//!
//! This module also owns reading a machine build's on-disk log for the panel's
//! inline log drawer (see [`LogTailCache`]): the status entries carry the
//! `/nix/var/log/nix/drvs/…` path each build is writing, and the UI fetches a
//! tail of it through `/api/global-log`.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use bytes::Bytes;
use nix_web_monitor_parser::global::parse_builds;
use nix_web_monitor_parser::{GlobalBuild, GlobalBuilds, MonitorState};
use tokio::process::Command;
use tokio::sync::{RwLock, broadcast};

use crate::broadcast_deltas;
use crate::proc_stats::BuildStatSampler;

/// How often the machine-wide build view is re-polled once detected. This
/// shells out to `nix` each tick, so a couple of seconds keeps the panel live
/// without a constant subprocess churn.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Back-off before re-probing after the subcommand comes back undetected (stock
/// nix). No point hammering a stock nix that will never grow the subcommand
/// mid-run, but re-probe occasionally so a nix upgrade during a long-lived UI
/// session is picked up.
const RETRY_INTERVAL: Duration = Duration::from_secs(30);

/// How often the parked probe re-checks for a first dashboard client.
const CLIENT_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// View status while the probe is parked with no dashboard client connected.
/// The parked view keeps the detection flag but clears the build rows: a late
/// joiner is seeded from the monitor snapshot, so it must not carry rows from
/// whenever the last client left. Their arrival un-parks the probe within a
/// poll interval and the next poll replaces this.
const UNWATCHED_STATUS: &str = "machine-wide polling idle -- resumes while the dashboard is open";

/// Cap on the decompressed log tail `/api/global-log` returns. A build log can
/// run to hundreds of megabytes; the panel's inline drawer only ever shows the
/// live tail, so everything older is dropped at a line boundary.
const LOG_TAIL_BYTES: usize = 64 * 1024;

/// Drop a cached log-tail decode not polled for this long. The drawer polls
/// every two seconds while open, so an entry this stale belongs to a closed
/// drawer (or a finished build) and is just parked decoder state and a tail
/// buffer nobody is reading.
const LOG_TAIL_STALE: Duration = Duration::from_secs(30);

/// Cap on concurrently cached log-tail decodes. One drawer is open per panel
/// at a time, so this only bounds the pathological case (many browsers, or a
/// scripted poller cycling workers) instead of ever mattering normally.
const LOG_TAIL_MAX_ENTRIES: usize = 8;

/// Run the machine-wide build probe until the task is aborted.
///
/// Never returns: the global view is a best-effort overlay, so any failure
/// becomes a status the panel shows (or a hidden panel) and the loop retries.
///
/// The poller runs only while at least one dashboard client is subscribed to
/// the delta feed: every tick shells out to `nix` and (on a busy machine)
/// sweeps procfs, for a view consumed nowhere else, so an unwatched poller is
/// pure subprocess churn. The gate sits *after* the poll, not before it:
/// detection -- does the patched subcommand exist, i.e. should the UI show the
/// pane at all -- is derived from this loop, so one pass must complete before
/// the first park for a later client's seeded snapshot to know whether the
/// pane exists.
pub async fn run_global_probe(monitor: Arc<RwLock<MonitorState>>, deltas: broadcast::Sender<Bytes>) {
    // Between polls, the sampler turns each build's pid into live cpu/rss
    // figures from procfs (see `proc_stats`); the two-second poll interval is
    // also the cpu averaging window.
    let mut sampler = BuildStatSampler::new();
    loop {
        let polled = poll_builds().await;
        // What this poll found: the parked view republishes it so the pane's
        // existence survives a park (the UI hides the pane on `false`).
        let detected = polled.is_some();
        let pause = if let Some(builds) = polled {
            let AnnotatedPoll { sampler: returned_sampler, builds } =
                annotate_off_runtime(sampler, builds).await;
            sampler = returned_sampler;
            let status = format!("{} active", builds.len());
            let global = GlobalBuilds {
                detected: true,
                builds,
                status,
            };
            publish(&monitor, &deltas, global).await;
            POLL_INTERVAL
        } else {
            // Undetected: publish the undetected view once (its `Default` carries
            // the "not available" status) so a later detection can flip the panel
            // on, then back off before re-probing. Drop the cpu baselines with
            // it: a transient failure after builds were sampled would otherwise
            // keep them across the (30-second) back-off, and a recovered row's
            // first cpu% would average the whole outage instead of the poll
            // window. Starting fresh makes recovery behave like a first-ever
            // sample: rss only, cpu% from the next poll.
            sampler = BuildStatSampler::new();
            publish(&monitor, &deltas, GlobalBuilds::default()).await;
            RETRY_INTERVAL
        };
        if deltas.receiver_count() == 0 {
            // Park until a dashboard client subscribes. Drop the cpu
            // baselines for the same reason as the undetected branch: kept
            // across a park, a row's
            // first cpu% after resuming would average the whole parked stretch
            // instead of the poll window.
            sampler = BuildStatSampler::new();
            publish(
                &monitor,
                &deltas,
                GlobalBuilds {
                    detected,
                    builds: Vec::new(),
                    status: UNWATCHED_STATUS.to_owned(),
                },
            )
            .await;
            while deltas.receiver_count() == 0 {
                tokio::time::sleep(CLIENT_POLL_INTERVAL).await;
            }
        } else {
            tokio::time::sleep(pause).await;
        }
    }
}

/// Annotate one poll's builds with procfs cpu/rss/generation figures on the
/// blocking pool.
///
/// The sampler's pass ([`BuildStatSampler::annotate`]) is synchronous
/// filesystem I/O: a full `/proc` sweep (one `stat` read per process on the
/// host) plus a `status` read per subtree pid. On a machine busy enough to
/// need this panel that can take long enough to stall an async runtime
/// worker, so it must not run inline in the probe task. The sampler owns the
/// previous tick's cpu baselines, so it moves into the closure and rides back
/// with the annotated list. Its idle short-circuit (no pids -> clear
/// baselines, no procfs reads) still applies inside the closure, so an idle
/// tick costs one no-op blocking task.
async fn annotate_off_runtime(
    mut sampler: BuildStatSampler,
    mut builds: Vec<GlobalBuild>,
) -> AnnotatedPoll {
    tokio::task::spawn_blocking(move || {
        sampler.annotate(&mut builds);
        AnnotatedPoll { sampler, builds }
    })
    .await
    // The closure neither panics nor is cancelled, so a join error is a bug;
    // the probe's contract is to never die, so recover with a fresh sampler
    // (one tick without cpu figures) rather than crash.
    .unwrap_or_else(|_| AnnotatedPoll {
        sampler: BuildStatSampler::new(),
        builds: Vec::new(),
    })
}

/// One poll's builds after the off-runtime annotate pass, with the sampler --
/// owner of the previous tick's cpu baselines -- riding back for the next
/// tick.
struct AnnotatedPoll {
    sampler: BuildStatSampler,
    builds: Vec<GlobalBuild>,
}

/// Run `nix store builds --json` and parse its output into a build list, or
/// `None` when the subcommand is unavailable (stock nix) or errored.
///
/// Detection is by *result*, not by exit-code text-matching: whatever variant of
/// the invocation yields a parseable JSON array wins. A stock nix prints an
/// "unknown command" / "unknown experimental feature" error to stderr (not a
/// JSON array), so every variant fails to parse and this returns `None` ->
/// undetected.
async fn poll_builds() -> Option<Vec<GlobalBuild>> {
    // The patched command is gated behind the `build-status-dir` experimental
    // feature, so the feature-enabling form is the one that normally succeeds
    // and goes first (one subprocess per tick on a patched nix). The plain form
    // is the fallback for a nix that rejects the unknown feature name but has
    // the command ungated or the feature enabled via nix.conf. The feature flag
    // must precede the subcommand: nix only honors it there, so a trailing flag
    // leaves the features off and a patched host misdetects as stock.
    const ATTEMPTS: [&[&str]; 2] = [
        &[
            "--extra-experimental-features",
            "nix-command build-status-dir",
            "store",
            "builds",
            "--json",
        ],
        &["--extra-experimental-features", "nix-command", "store", "builds", "--json"],
    ];
    for args in ATTEMPTS {
        if let Some(builds) = try_builds(args).await {
            return Some(builds);
        }
    }
    None
}

/// Run one `nix` argument variant and return the parsed builds if its stdout is
/// a JSON build array. Any spawn failure, or output that is not a build array,
/// yields `None` so the caller falls through to the next variant / undetected.
async fn try_builds(args: &[&str]) -> Option<Vec<GlobalBuild>> {
    let output = Command::new("nix").args(args).output().await.ok()?;
    // Parse stdout regardless of exit status: a patched nix might print the array
    // and still exit nonzero on some warning, and a stock nix prints its error to
    // stderr with empty stdout, so the parse is the real detector.
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_builds(stdout.trim()).ok()
}

/// Push a machine-wide build view to the monitor and broadcast the change.
/// `set_global` skips a no-op, so an unchanged view puts no frame on the wire.
async fn publish(
    monitor: &Arc<RwLock<MonitorState>>,
    deltas: &broadcast::Sender<Bytes>,
    global: GlobalBuilds,
) {
    monitor.write().await.set_global(global);
    let _ = broadcast_deltas(monitor, deltas).await;
}

/// Resolve an active machine build's recorded log file by exact worker.
///
/// This is the gate on `/api/global-log`: the server only ever opens paths the
/// status directory itself advertised for a *currently active* build, so the
/// endpoint cannot be steered at arbitrary files.
///
/// `start_ticks` is the worker's kernel start-time generation as the client
/// last saw it (procfs ticks on Linux, the sysctl start timestamp on macOS),
/// matched exactly (`None` included): `start_time` is whole seconds, so the
/// generation is what keeps a pid recycled for the same drv within one second
/// from resolving to its predecessor's log. `None` only matches a worker the
/// sampler could not see (already gone when sampled) -- it is never a
/// wildcard over a sampled one.
pub async fn log_file_for(
    monitor: &Arc<RwLock<MonitorState>>,
    drv_path: &str,
    pid: i64,
    start_time: i64,
    start_ticks: Option<u64>,
) -> Option<PathBuf> {
    monitor
        .read()
        .await
        .global
        .builds
        .iter()
        .find(|build| {
            build.drv_path.as_deref() == Some(drv_path)
                && build.pid == Some(pid)
                && build.start_time == Some(start_time)
                && build.start_ticks == start_ticks
        })
        .and_then(|build| build.log_file.as_deref().map(PathBuf::from))
}

/// Exact worker identity a `/api/global-log` poll targets, and the key its
/// incremental decode state is cached under. Mirrors the components
/// [`log_file_for`] gates on, so a recycled pid or a new worker generation
/// never resumes its predecessor's decoder.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LogWorkerKey {
    pub drv_path: String,
    pub pid: i64,
    pub start_time: i64,
    pub start_ticks: Option<u64>,
}

/// Incremental log-tail reader behind `/api/global-log`.
///
/// Nix compresses build logs *while writing them* (`.drv.bz2` under
/// `/nix/var/log/nix/drvs`), so a live log is a truncated bzip2 stream. `nix
/// log` refuses such a stream, which is why the server reads the file itself.
/// The drawer polls every two seconds while open, and decoding the whole
/// stream from byte 0 on each poll made every tick cost the full compressed
/// length in CPU and disk on a big active log. So the cache keeps, per polled
/// worker, the live decode position: how many compressed bytes have been fed,
/// the persistent [`bzip2::Decompress`] (a resumable streaming state machine
/// that carries mid-block state across calls), and the rolling decoded tail.
/// A poll then reads and decodes only newly appended compressed bytes.
///
/// Entries evict once a drawer stops polling ([`LOG_TAIL_STALE`]) and the map
/// is capped at [`LOG_TAIL_MAX_ENTRIES`] (least-recently-polled goes first).
/// Cheap to clone: handlers share one map behind the [`Arc`].
#[derive(Clone, Default)]
pub struct LogTailCache {
    entries: Arc<LogTailEntries>,
}

type LogTailEntries = Mutex<HashMap<LogWorkerKey, LogTailEntry>>;

impl LogTailCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Read the tail of one build's on-disk log, decompressing (incrementally,
    /// resuming `key`'s cached decode) when needed. The blocking read and
    /// decompression run off the async executor, consistent with the sampler.
    ///
    /// # Errors
    ///
    /// Returns the underlying I/O error when the file cannot be read (most
    /// commonly [`std::io::ErrorKind::NotFound`] before the builder's first
    /// output flush creates it).
    pub async fn read_log_tail(&self, key: LogWorkerKey, path: PathBuf) -> std::io::Result<String> {
        let entries = Arc::clone(&self.entries);
        tokio::task::spawn_blocking(move || read_tail_blocking(&entries, key, path))
            .await
            // The closure neither panics nor is cancelled; a join error here is a bug.
            .unwrap_or_else(|join_error| {
                Err(std::io::Error::other(format!(
                    "log read task failed: {join_error}"
                )))
            })
    }
}

/// One poll's blocking work: the plain-log path seeks and reads the tail
/// directly (no state worth caching); the `.bz2` path resumes the worker's
/// cached decode. The cache entry is *taken out* of the map for the duration
/// of the decode so the lock is never held across file I/O; a concurrent poll
/// for the same worker (two browsers -- the panel itself skips overlapping
/// polls) just starts a fresh decode and the last writer wins.
fn read_tail_blocking(
    entries: &LogTailEntries,
    key: LogWorkerKey,
    path: PathBuf,
) -> std::io::Result<String> {
    if path.extension().is_none_or(|extension| extension != "bz2") {
        let decoded = read_plain_tail(File::open(&path)?)?;
        return Ok(tail_lines(
            &decoded.bytes,
            LOG_TAIL_BYTES,
            decoded.prefix_dropped,
        ));
    }
    let taken = lock(entries).remove(&key);
    let mut entry = match taken {
        // Same worker, same recorded path: resume. A changed path (should not
        // happen within one worker generation) restarts from scratch.
        Some(entry) if entry.path == path => entry,
        _ => LogTailEntry::new(path),
    };
    match entry.advance() {
        Ok(()) => {
            let text = tail_lines(&entry.tail, LOG_TAIL_BYTES, entry.prefix_dropped);
            entry.last_polled = Instant::now();
            store(entries, key, entry);
            Ok(text)
        }
        // Reading failed; the entry stays dropped so the next poll starts clean.
        Err(error) => Err(error),
    }
}

/// Re-insert a polled entry, sweeping stale neighbors and enforcing the size
/// cap (least recently polled evicts first).
fn store(entries: &LogTailEntries, key: LogWorkerKey, entry: LogTailEntry) {
    let mut map = lock(entries);
    let now = Instant::now();
    map.retain(|_, cached| now.duration_since(cached.last_polled) < LOG_TAIL_STALE);
    map.insert(key, entry);
    while map.len() > LOG_TAIL_MAX_ENTRIES {
        let Some(oldest) = map
            .iter()
            .min_by_key(|(_, cached)| cached.last_polled)
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        map.remove(&oldest);
    }
    drop(map);
}

fn lock(entries: &LogTailEntries) -> MutexGuard<'_, HashMap<LogWorkerKey, LogTailEntry>> {
    // The map is only touched outside the decode work, so a poisoning panic
    // while holding the lock has no bug to hide; recover the map.
    entries
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// White-box peeks the incrementality tests assert on (the served text alone
/// cannot show that a poll skipped re-decoding).
#[cfg(test)]
impl LogTailCache {
    fn entry_count(&self) -> usize {
        lock(&self.entries).len()
    }

    /// Compressed bytes the worker's cached decode has consumed so far.
    fn consumed_for(&self, key: &LogWorkerKey) -> Option<u64> {
        lock(&self.entries).get(key).map(|entry| entry.consumed)
    }

    /// Age a cached entry as if its drawer stopped polling `by` ago. False
    /// when the entry is missing or the monotonic clock cannot represent the
    /// backdated instant (host uptime shorter than `by`).
    fn backdate(&self, key: &LogWorkerKey, by: Duration) -> bool {
        let mut map = lock(&self.entries);
        match (map.get_mut(key), Instant::now().checked_sub(by)) {
            (Some(entry), Some(past)) => {
                entry.last_polled = past;
                true
            }
            _ => false,
        }
    }
}

/// One worker's live decode: how far into the compressed file the decoder has
/// been fed, the decoder itself (holding any mid-block state), and the rolling
/// decoded tail.
struct LogTailEntry {
    path: PathBuf,
    /// Compressed bytes already fed to `decoder`.
    consumed: u64,
    decoder: bzip2::Decompress,
    /// Rolling decoded tail; capped near [`LOG_TAIL_BYTES`] so a huge log
    /// never balloons memory.
    tail: Vec<u8>,
    /// Whether the rolling cap discarded earlier decoded bytes (so `tail` no
    /// longer starts at the log's true beginning and the serving side must
    /// cut to a line boundary).
    prefix_dropped: bool,
    /// The decoder is done (stream footer, or undecodable bytes): appended
    /// bytes can never decode further, so polls stop reading the file.
    finished: bool,
    last_polled: Instant,
}

impl LogTailEntry {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            consumed: 0,
            decoder: bzip2::Decompress::new(false),
            tail: Vec::new(),
            prefix_dropped: false,
            finished: false,
            last_polled: Instant::now(),
        }
    }

    /// Feed the compressed bytes appended since the last poll through the
    /// persistent decoder. An unchanged file is a metadata check and nothing
    /// else; a shrunk file (rotated or replaced -- not expected of nix's
    /// append-only drv logs, but never trust an offset into a changed file)
    /// restarts the decode from byte 0.
    fn advance(&mut self) -> std::io::Result<()> {
        let mut file = File::open(&self.path)?;
        let length = file.metadata()?.len();
        if length < self.consumed {
            *self = Self::new(std::mem::take(&mut self.path));
        }
        if self.finished || length == self.consumed {
            return Ok(());
        }
        file.seek(SeekFrom::Start(self.consumed))?;
        let mut reader = BufReader::new(file);
        let mut chunk = vec![0_u8; 64 * 1024];
        loop {
            let read = reader.read(&mut chunk)?;
            if read == 0 {
                return Ok(());
            }
            self.consumed += read as u64;
            self.feed(&chunk[..read]);
            if self.finished {
                return Ok(());
            }
        }
    }

    /// Run one chunk of compressed input through the decoder, appending
    /// whatever decodes to the rolling tail. Tolerant like the whole-file
    /// decode this replaced: a decode error just finishes the readable log,
    /// because everything decoded so far *is* the live log.
    ///
    /// bzip2 is a block format (the BWT inverse needs the whole block), so
    /// the live tail advances one *completed* block at a time: nix compresses
    /// at the default 900 KB block size, meaning a quiet build's log decodes
    /// to nothing until its first 900 KB of output. A trailing partial block
    /// parks inside the decoder's state and completes on a later poll.
    fn feed(&mut self, input: &[u8]) {
        let mut output = vec![0_u8; 64 * 1024];
        let mut fed = 0;
        while fed < input.len() {
            let before_in = self.decoder.total_in();
            let before_out = self.decoder.total_out();
            let status = self.decoder.decompress(&input[fed..], &mut output);
            let consumed = usize::try_from(self.decoder.total_in() - before_in)
                .expect("chunk-bounded input consumption fits usize");
            let produced = usize::try_from(self.decoder.total_out() - before_out)
                .expect("buffer-bounded output fits usize");
            fed += consumed;
            if produced > 0 {
                self.push_tail(&output[..produced]);
            }
            match status {
                // Footer seen (build finished cleanly) or the bytes are not
                // decodable bzip2: either way the readable log ends here.
                Ok(bzip2::Status::StreamEnd) | Err(_) => {
                    self.finished = true;
                    return;
                }
                Ok(_) => {
                    // With input pending and a whole spare output buffer the
                    // decoder always makes progress; treat a no-progress
                    // return as the end rather than spinning on it.
                    if consumed == 0 && produced == 0 {
                        self.finished = true;
                        return;
                    }
                }
            }
        }
    }

    /// Append decoded bytes, keeping only the newest [`LOG_TAIL_BYTES`]-ish.
    fn push_tail(&mut self, bytes: &[u8]) {
        self.tail.extend_from_slice(bytes);
        if self.tail.len() > LOG_TAIL_BYTES * 2 {
            self.tail.drain(..self.tail.len() - LOG_TAIL_BYTES);
            self.prefix_dropped = true;
        }
    }
}

/// A bounded read of an on-disk log: the newest bytes, plus whether earlier
/// bytes were discarded (so the buffer no longer starts at the log's true
/// beginning and the caller must cut to a line boundary).
struct DecodedTail {
    bytes: Vec<u8>,
    prefix_dropped: bool,
}

/// Read at most the final [`LOG_TAIL_BYTES`] from an uncompressed log. Seeking
/// first keeps both I/O and memory bounded even when a build has emitted
/// hundreds of megabytes.
fn read_plain_tail(mut file: File) -> std::io::Result<DecodedTail> {
    let length = file.metadata()?.len();
    let keep = u64::try_from(LOG_TAIL_BYTES).expect("log tail cap fits u64");
    let start = length.saturating_sub(keep);
    file.seek(SeekFrom::Start(start))?;

    let capacity = usize::try_from(length - start).expect("bounded log tail length fits usize");
    let mut bytes = Vec::with_capacity(capacity);
    file.take(keep).read_to_end(&mut bytes)?;
    Ok(DecodedTail {
        bytes,
        prefix_dropped: start > 0,
    })
}

/// The last `keep` bytes as text, cut forward to a line boundary so the tail
/// never opens mid-line. `prefix_dropped` marks a buffer whose head was already
/// discarded upstream (the decoder's rolling cap cuts at an arbitrary byte), so
/// the cut applies even when the buffer is under `keep`. Lossy decode: build
/// logs are not guaranteed UTF-8.
fn tail_lines(bytes: &[u8], keep: usize, prefix_dropped: bool) -> String {
    let start = bytes.len().saturating_sub(keep);
    let mut tail = &bytes[start..];
    if (start > 0 || prefix_dropped)
        && let Some(newline) = tail.iter().position(|&byte| byte == b'\n')
    {
        tail = &tail[newline + 1..];
    }
    String::from_utf8_lossy(tail).into_owned()
}

#[cfg(test)]
mod tests {
    use nix_web_monitor_parser::{GlobalBuildKind, MonitorState};

    use super::*;

    /// Compress `text` as a bzip2 stream. `level` also sets the block size
    /// (`level * 100 KB`): nix writes at the default level 9, but the
    /// truncation test uses level 1 so a modest fixture spans several blocks.
    fn compress_bzip2(text: &str, level: u32) -> Vec<u8> {
        use std::io::Write;
        let mut encoder = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::new(level));
        encoder.write_all(text.as_bytes()).expect("compress log");
        encoder.finish().expect("finish bzip2 stream")
    }

    /// End-to-end of the parse-into-state path the probe drives: a sample
    /// `nix store builds --json` payload folds into a `GlobalBuilds` with the
    /// right rows and why-chain, and `set_global` broadcasts a `GlobalSet` delta.
    #[test]
    fn sample_payload_folds_into_state_with_why_chain() {
        let json = r#"[
            {
                "drvPath": "/nix/store/aaa-foo.drv",
                "outputs": ["out"],
                "type": "build",
                "pid": 4242,
                "startTime": 1720200000,
                "user": "alice",
                "uid": 1000,
                "logFile": "/nix/var/log/nix/drvs/ab/cdfoo.drv.bz2",
                "why": {
                    "rootDrvPath": "/nix/store/root-app.drv",
                    "chain": ["/nix/store/root-app.drv", "/nix/store/aaa-foo.drv"],
                    "cause": "outputsMissing"
                }
            },
            {
                "storePath": "/nix/store/bbb-bar",
                "type": "substitution",
                "why": { "cause": "outputInvalid" }
            }
        ]"#;
        let builds = parse_builds(json).expect("sample payload parses");
        let global = GlobalBuilds {
            detected: true,
            status: format!("{} active", builds.len()),
            builds,
        };

        assert_eq!(global.builds.len(), 2);
        assert_eq!(global.status, "2 active");
        assert_eq!(global.builds[0].kind, GlobalBuildKind::Build);
        assert_eq!(global.builds[0].user.as_deref(), Some("alice"));
        assert_eq!(
            global.builds[0].why.root_drv_path.as_deref(),
            Some("/nix/store/root-app.drv")
        );
        assert_eq!(global.builds[0].why.chain.len(), 2);
        assert_eq!(global.builds[1].kind, GlobalBuildKind::Substitution);
        assert_eq!(
            global.builds[1].store_path.as_deref(),
            Some("/nix/store/bbb-bar")
        );

        // Folding into state emits exactly one GlobalSet delta.
        let mut state = MonitorState::default();
        state.set_global(global.clone());
        let deltas = state.drain_deltas();
        assert_eq!(deltas.len(), 1);
        assert!(matches!(
            deltas.first(),
            Some(nix_web_monitor_parser::Delta::GlobalSet { .. })
        ));
        assert!(state.snapshot().global.detected);
        assert_eq!(state.snapshot().global.builds.len(), 2);

        // Re-setting the identical view is a no-op (no redundant frame).
        state.set_global(global);
        assert!(state.drain_deltas().is_empty());
    }

    /// Poll the monitor until its global status satisfies `accept`, panicking
    /// after a generous deadline (the probe's first pass shells out to
    /// whatever `nix` the host has, so timing is not deterministic).
    async fn wait_for_global_status(
        monitor: &Arc<RwLock<MonitorState>>,
        accept: impl Fn(&str) -> bool,
        what: &str,
    ) -> GlobalBuilds {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let global = monitor.read().await.global.clone();
            if accept(&global.status) {
                return global;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}; last status: {:?}",
                global.status
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// With no dashboard client subscribed, the probe parks after its single
    /// detection pass instead of polling `nix` forever: the parked view
    /// carries the unwatched status and no build rows, so a late joiner is
    /// never seeded with rows from whenever the last client left.
    #[tokio::test]
    async fn probe_parks_without_clients() {
        let monitor = Arc::new(RwLock::new(MonitorState::default()));
        let (deltas, _) = broadcast::channel(8);
        let probe = tokio::spawn(run_global_probe(Arc::clone(&monitor), deltas));
        let parked = wait_for_global_status(
            &monitor,
            |status| status == UNWATCHED_STATUS,
            "the probe to park",
        )
        .await;
        probe.abort();
        assert!(parked.builds.is_empty(), "the parked view carries no stale rows");
    }

    /// A first client subscribing un-parks the probe within a client-poll
    /// tick: the next poll replaces the parked view (with "N active" on a
    /// patched nix or the undetected default on stock -- either way the
    /// unwatched status goes away).
    #[tokio::test]
    async fn probe_resumes_when_a_client_subscribes() {
        let monitor = Arc::new(RwLock::new(MonitorState::default()));
        let (deltas, _) = broadcast::channel(8);
        let probe = tokio::spawn(run_global_probe(Arc::clone(&monitor), deltas.clone()));
        wait_for_global_status(&monitor, |status| status == UNWATCHED_STATUS, "the probe to park")
            .await;

        let client = deltas.subscribe();
        wait_for_global_status(
            &monitor,
            |status| status != UNWATCHED_STATUS,
            "the probe to resume polling",
        )
        .await;
        probe.abort();
        drop(client);
    }

    /// Run `compressed` through a fresh persistent decoder in `pieces` roughly
    /// equal slices, the way successive polls feed appended bytes.
    fn feed_pieces(compressed: &[u8], pieces: usize) -> LogTailEntry {
        let mut entry = LogTailEntry::new(PathBuf::new());
        let size = compressed.len().div_ceil(pieces).max(1);
        for piece in compressed.chunks(size) {
            entry.feed(piece);
        }
        entry
    }

    /// A distinct worker identity per `tag`, for cache-keyed tests.
    fn worker_key(tag: i64) -> LogWorkerKey {
        LogWorkerKey {
            drv_path: format!("/nix/store/{tag}-fixture.drv"),
            pid: tag,
            start_time: 1_720_200_000 + tag,
            start_ticks: u64::try_from(tag).ok(),
        }
    }

    /// A complete `.bz2` log round-trips; the tail keeps the end of the log.
    #[test]
    fn bz2_log_round_trips_and_tails() {
        let text = "configuring\nbuilding\ninstalling\n";
        let compressed = compress_bzip2(text, 9);
        let entry = feed_pieces(&compressed, 1);
        assert_eq!(entry.tail, text.as_bytes());
        assert!(!entry.prefix_dropped, "a small log keeps its whole prefix");
        assert!(entry.finished, "the footer ends the decode");
        assert_eq!(tail_lines(text.as_bytes(), 1 << 20, false), text);
    }

    /// The decode-state table: how each on-disk shape a poll can meet -- cut
    /// mid-stream, cut inside the first block, not bzip2 at all -- lands in
    /// (decoded something?, finished?), for every way the bytes might be
    /// sliced across polls. The truncated cases are the live-write shape `nix
    /// log` refuses and the reason the server decompresses the file itself;
    /// slicing must never change what decodes, because a real drawer feeds
    /// the same stream in poll-sized increments. Level 1 (100 KB blocks)
    /// keeps the multi-block fixture small; nix's level-9 stream behaves
    /// identically at 900 KB granularity.
    #[test]
    fn decode_states_hold_across_poll_slicings() {
        let line = "log line with some incompressible entropy 8d1f4a2c\n";
        // ~500 KB uncompressed -> ~5 level-1 blocks, and enough decoded output
        // to exercise the rolling cap.
        let multi_block = compress_bzip2(&line.repeat(10_000), 1);
        let single_block = compress_bzip2(&"short log\n".repeat(100), 9);
        // (fixture, decodes something?, finished?) per on-disk shape.
        let cases: [(&str, &[u8], bool, bool); 4] = [
            ("mid-stream cut", &multi_block[..multi_block.len() / 2], true, false),
            ("first block cut", &single_block[..single_block.len() / 2], false, false),
            ("not bzip2", b"error: not a log", false, true),
            ("complete stream", &multi_block, true, true),
        ];
        for (name, bytes, decodes, finished) in cases {
            let reference = feed_pieces(bytes, 1);
            assert_eq!(!reference.tail.is_empty(), decodes, "{name}: decoded output");
            assert_eq!(reference.finished, finished, "{name}: decoder state");
            if decodes {
                assert!(
                    String::from_utf8_lossy(&reference.tail).contains("entropy 8d1f4a2c"),
                    "{name}: decoded bytes are real log content"
                );
            }
            // The rolling buffer's exact length varies with when its cap
            // triggered, so the invariant is what gets *served*: the
            // line-aligned tail, and the decoder's terminal state.
            let served = tail_lines(&reference.tail, LOG_TAIL_BYTES, reference.prefix_dropped);
            for pieces in [2, 7, 64] {
                let sliced = feed_pieces(bytes, pieces);
                assert_eq!(
                    tail_lines(&sliced.tail, LOG_TAIL_BYTES, sliced.prefix_dropped),
                    served,
                    "{name} in {pieces} pieces"
                );
                assert_eq!(sliced.finished, reference.finished, "{name} in {pieces} pieces");
            }
        }
    }

    /// The tail is bounded and opens on a line boundary, never mid-line.
    #[test]
    fn tail_is_bounded_and_line_aligned() {
        use std::fmt::Write;
        let text = (0..1000).fold(String::new(), |mut log, i| {
            let _ = writeln!(log, "line {i}");
            log
        });
        let tail = tail_lines(text.as_bytes(), 100, false);
        assert!(tail.len() <= 100);
        assert!(tail.starts_with("line "), "tail begins at a line start");
        assert!(tail.ends_with("line 999\n"), "tail keeps the newest lines");
    }

    /// When the decoder already dropped the head (rolling cap), the cut to a
    /// line boundary must happen even though the buffer is under the cap:
    /// the buffer's first line is a fragment cut at an arbitrary byte.
    #[test]
    fn dropped_prefix_forces_line_boundary_cut() {
        assert_eq!(tail_lines(b"ragment\nwhole line\n", 1 << 20, true), "whole line\n");
        // Without the marker the same buffer is a complete log and keeps line 1.
        assert_eq!(
            tail_lines(b"ragment\nwhole line\n", 1 << 20, false),
            "ragment\nwhole line\n"
        );
    }

    /// `log_file_for` only resolves builds the status view currently lists:
    /// the drv must be active *and* carry a recorded log, and the whole worker
    /// identity (pid, start second, start-tick generation) must match exactly.
    /// This is the arbitrary-file-read gate on `/api/global-log`.
    #[tokio::test]
    async fn log_file_for_resolves_only_active_builds() {
        // One row of the identity table below: (drv, pid, start second,
        // ticks) -> expected log path.
        type Case = (&'static str, i64, i64, Option<u64>, Option<&'static str>);
        let with_log = GlobalBuild {
            drv_path: Some("/nix/store/aaa-foo.drv".to_owned()),
            pid: Some(11),
            start_time: Some(100),
            start_ticks: Some(9000),
            log_file: Some("/nix/var/log/nix/drvs/ab/cdfoo.drv.bz2".to_owned()),
            ..GlobalBuild::default()
        };
        let other_worker = GlobalBuild {
            drv_path: Some("/nix/store/aaa-foo.drv".to_owned()),
            pid: Some(12),
            start_time: Some(200),
            start_ticks: Some(9500),
            log_file: Some("/nix/var/log/nix/drvs/ab/cdfoo.drv.2.bz2".to_owned()),
            ..GlobalBuild::default()
        };
        // The sampler could not see this worker (gone between the status
        // poll and the sample): the whole identity is (pid, start second,
        // no generation) and still matches exactly.
        let unsampled = GlobalBuild {
            drv_path: Some("/nix/store/ccc-baz.drv".to_owned()),
            pid: Some(21),
            start_time: Some(300),
            log_file: Some("/nix/var/log/nix/drvs/cc/cbaz.drv.bz2".to_owned()),
            ..GlobalBuild::default()
        };
        let without_log = GlobalBuild {
            drv_path: Some("/nix/store/bbb-bar.drv".to_owned()),
            ..GlobalBuild::default()
        };
        let mut state = MonitorState::default();
        state.set_global(GlobalBuilds {
            detected: true,
            builds: vec![with_log, other_worker, unsampled, without_log],
            status: "4 active".to_owned(),
        });
        let monitor = Arc::new(RwLock::new(state));

        // Table of identity cases; the misses each break exactly one
        // identity component.
        let cases: [Case; 9] = [
            ("/nix/store/aaa-foo.drv", 11, 100, Some(9000), Some("/nix/var/log/nix/drvs/ab/cdfoo.drv.bz2")),
            ("/nix/store/aaa-foo.drv", 12, 200, Some(9500), Some("/nix/var/log/nix/drvs/ab/cdfoo.drv.2.bz2")),
            ("/nix/store/ccc-baz.drv", 21, 300, None, Some("/nix/var/log/nix/drvs/cc/cbaz.drv.bz2")),
            // A recycled pid must not resolve its predecessor's log: neither
            // across start seconds nor -- same second -- across generations.
            ("/nix/store/aaa-foo.drv", 11, 101, Some(9000), None),
            ("/nix/store/aaa-foo.drv", 11, 100, Some(9001), None),
            // No wildcard: omitted ticks never resolve a sampled worker.
            ("/nix/store/aaa-foo.drv", 11, 100, None, None),
            ("/nix/store/aaa-foo.drv", 13, 100, Some(9000), None),
            ("/nix/store/bbb-bar.drv", 11, 100, Some(9000), None),
            ("/etc/passwd", 11, 100, Some(9000), None),
        ];
        for (drv, pid, start, ticks, expected) in cases {
            assert_eq!(
                log_file_for(&monitor, drv, pid, start, ticks).await,
                expected.map(PathBuf::from),
                "identity ({drv}, {pid}, {start}, {ticks:?})"
            );
        }
    }

    /// A large plain log is read from its end and still opens on a whole line.
    /// The plain path never caches: no decode state is worth keeping.
    #[tokio::test]
    async fn read_log_tail_seeks_to_plain_file_tail() {
        let dir = std::env::temp_dir().join(format!("nwm-global-plain-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        let path = dir.join("test.drv");
        let prefix = "old line\n".repeat(LOG_TAIL_BYTES);
        std::fs::write(&path, format!("{prefix}newest line\n")).expect("write fixture log");

        let cache = LogTailCache::new();
        let tail = cache
            .read_log_tail(worker_key(1), path)
            .await
            .expect("fixture log reads");
        assert!(tail.len() <= LOG_TAIL_BYTES);
        assert!(tail.starts_with("old line\n"), "tail starts at a line boundary");
        assert!(tail.ends_with("newest line\n"));
        assert_eq!(cache.entry_count(), 0, "plain logs leave no decode state");

        std::fs::remove_dir_all(&dir).expect("clean scratch dir");
    }

    /// Reading a real compressed file end-to-end through the async entry point.
    #[tokio::test]
    async fn read_log_tail_reads_compressed_file() {
        let dir = std::env::temp_dir().join(format!("nwm-global-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        let path = dir.join("test.drv.bz2");
        std::fs::write(&path, compress_bzip2("hello from the builder\n", 9))
            .expect("write fixture log");

        let tail = LogTailCache::new()
            .read_log_tail(worker_key(1), path)
            .await
            .expect("fixture log reads");
        assert_eq!(tail, "hello from the builder\n");

        std::fs::remove_dir_all(&dir).expect("clean scratch dir");
    }

    /// A missing log file (builder has not flushed yet) is a clean `NotFound`,
    /// which the endpoint maps to 404 rather than an empty 200.
    #[tokio::test]
    async fn read_log_tail_missing_file_is_not_found() {
        let error = LogTailCache::new()
            .read_log_tail(worker_key(1), PathBuf::from("/nonexistent/nwm-test.drv.bz2"))
            .await
            .expect_err("missing file errors");
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }

    /// The drawer's poll pattern end-to-end: the first poll decodes the
    /// completed blocks, an unchanged file is a metadata check (nothing new
    /// consumed), and appended compressed bytes decode *incrementally* -- the
    /// consumed offset only ever moves forward, never back to byte 0.
    #[tokio::test]
    async fn cached_bz2_tail_advances_across_appends_without_redecoding() {
        let dir = std::env::temp_dir().join(format!("nwm-global-incr-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        let path = dir.join("test.drv.bz2");
        let line = "log line with some incompressible entropy 8d1f4a2c\n";
        let text = line.repeat(10_000);
        let compressed = compress_bzip2(&text, 1);
        let half = compressed.len() / 2;
        std::fs::write(&path, &compressed[..half]).expect("write live half");

        let cache = LogTailCache::new();
        let key = worker_key(7);
        let first = cache
            .read_log_tail(key.clone(), path.clone())
            .await
            .expect("live log reads");
        assert!(
            first.contains("entropy 8d1f4a2c"),
            "completed blocks serve while the build writes"
        );
        assert_eq!(cache.consumed_for(&key), Some(half as u64));

        // Unchanged file: the poll consumes nothing new and serves the same tail.
        let unchanged = cache
            .read_log_tail(key.clone(), path.clone())
            .await
            .expect("unchanged log reads");
        assert_eq!(unchanged, first);
        assert_eq!(cache.consumed_for(&key), Some(half as u64));

        // The builder appends the rest; only the new bytes feed the decoder.
        std::fs::write(&path, &compressed).expect("append the rest");
        let full = cache
            .read_log_tail(key.clone(), path.clone())
            .await
            .expect("finished log reads");
        assert!(full.ends_with(line), "the tail advances to the newest lines");
        assert_eq!(cache.consumed_for(&key), Some(compressed.len() as u64));

        std::fs::remove_dir_all(&dir).expect("clean scratch dir");
    }

    /// A file that shrank under a cached offset (rotation/replacement; nix's
    /// drv logs are append-only, but an offset into a changed file is never
    /// trustworthy) restarts the decode from byte 0 instead of serving a
    /// spliced tail.
    #[tokio::test]
    async fn cached_bz2_tail_restarts_when_the_file_shrinks() {
        let dir = std::env::temp_dir().join(format!("nwm-global-shrink-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        let path = dir.join("test.drv.bz2");
        std::fs::write(&path, compress_bzip2(&"first generation\n".repeat(50), 9))
            .expect("write first log");

        let cache = LogTailCache::new();
        let key = worker_key(3);
        let first = cache
            .read_log_tail(key.clone(), path.clone())
            .await
            .expect("first log reads");
        assert!(first.contains("first generation"));

        std::fs::write(&path, compress_bzip2("replacement\n", 9)).expect("replace with shorter");
        let second = cache
            .read_log_tail(key.clone(), path.clone())
            .await
            .expect("replacement reads");
        assert_eq!(second, "replacement\n");

        std::fs::remove_dir_all(&dir).expect("clean scratch dir");
    }

    /// Cache hygiene: entries evict once their drawer stops polling (the
    /// 30-second staleness sweep) and the map never grows past its cap, with
    /// the least recently polled worker evicted first.
    #[tokio::test]
    async fn cache_sweeps_stale_entries_and_caps_its_size() {
        let dir = std::env::temp_dir().join(format!("nwm-global-evict-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        let path = dir.join("test.drv.bz2");
        std::fs::write(&path, compress_bzip2("shared fixture log\n", 9)).expect("write fixture");

        let cache = LogTailCache::new();
        let stale = worker_key(0);
        cache
            .read_log_tail(stale.clone(), path.clone())
            .await
            .expect("stale worker reads");
        // Age the first worker past the staleness window, then let any other
        // poll's sweep collect it. Skipped only when the host's monotonic
        // clock is younger than the window (no instant that far back exists).
        if cache.backdate(&stale, LOG_TAIL_STALE + Duration::from_secs(1)) {
            cache
                .read_log_tail(worker_key(1), path.clone())
                .await
                .expect("fresh worker reads");
            assert_eq!(cache.consumed_for(&stale), None, "stale entry swept");
        }

        // Overflow the cap with distinct workers: the map stays bounded.
        let keys: Vec<LogWorkerKey> = (10..=(10 + i64::try_from(LOG_TAIL_MAX_ENTRIES).expect("small cap")))
            .map(worker_key)
            .collect();
        for key in &keys {
            cache
                .read_log_tail(key.clone(), path.clone())
                .await
                .expect("worker reads");
        }
        assert!(cache.entry_count() <= LOG_TAIL_MAX_ENTRIES, "cache stays capped");
        assert!(
            cache.consumed_for(keys.last().expect("nonempty keys")).is_some(),
            "the newest poll survives the eviction"
        );

        std::fs::remove_dir_all(&dir).expect("clean scratch dir");
    }
}
