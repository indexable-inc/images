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
//! undetected and the UI hides the panel. Like the daemon tracer, it never
//! returns and never panics: every failure becomes a status string, and the
//! loop backs off and retries so a mid-session nix upgrade is eventually picked
//! up.
//!
//! This module also owns reading a machine build's on-disk log for the panel's
//! inline log drawer (see [`read_log_tail`]): the status entries carry the
//! `/nix/var/log/nix/drvs/…` path each build is writing, and the UI fetches a
//! tail of it through `/api/global-log`.

use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use nix_web_monitor_parser::global::parse_builds;
use nix_web_monitor_parser::{GlobalBuild, GlobalBuilds, MonitorState};
use tokio::process::Command;
use tokio::sync::{RwLock, broadcast};

use crate::broadcast_deltas;

/// How often the machine-wide build view is re-polled once detected. Slower than
/// the daemon tracer's one-second sample: this shells out to `nix` each tick, so
/// a couple of seconds keeps the panel live without a constant subprocess churn.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Back-off before re-probing after the subcommand comes back undetected (stock
/// nix). No point hammering a stock nix that will never grow the subcommand
/// mid-run, but re-probe occasionally so a nix upgrade during a long-lived UI
/// session is picked up.
const RETRY_INTERVAL: Duration = Duration::from_secs(30);

/// Cap on the decompressed log tail `/api/global-log` returns. A build log can
/// run to hundreds of megabytes; the panel's inline drawer only ever shows the
/// live tail, so everything older is dropped at a line boundary.
const LOG_TAIL_BYTES: usize = 64 * 1024;

/// Run the machine-wide build probe until the task is aborted.
///
/// Never returns: like [`run_daemon_probe`](crate::daemon::run_daemon_probe),
/// the global view is a best-effort overlay, so any failure becomes a status the
/// panel shows (or a hidden panel) and the loop retries.
pub async fn run_global_probe(monitor: Arc<RwLock<MonitorState>>, deltas: broadcast::Sender<Bytes>) {
    loop {
        let Some(builds) = poll_builds().await else {
            // Undetected: publish the undetected view once (its `Default` carries
            // the "not available" status) so a later detection can flip the panel
            // on, then back off before re-probing.
            publish(&monitor, &deltas, GlobalBuilds::default()).await;
            tokio::time::sleep(RETRY_INTERVAL).await;
            continue;
        };
        let status = format!("{} active", builds.len());
        let global = GlobalBuilds {
            detected: true,
            builds,
            status,
        };
        publish(&monitor, &deltas, global).await;
        tokio::time::sleep(POLL_INTERVAL).await;
    }
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
    // the command ungated or the feature enabled via nix.conf.
    const ATTEMPTS: [&[&str]; 2] = [
        &[
            "store",
            "builds",
            "--json",
            "--extra-experimental-features",
            "nix-command build-status-dir",
        ],
        &["store", "builds", "--json", "--extra-experimental-features", "nix-command"],
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
/// Mirrors the daemon probe's `publish_status`: `set_global` skips a no-op, so an
/// unchanged view puts no frame on the wire.
async fn publish(
    monitor: &Arc<RwLock<MonitorState>>,
    deltas: &broadcast::Sender<Bytes>,
    global: GlobalBuilds,
) {
    monitor.write().await.set_global(global);
    let _ = broadcast_deltas(monitor, deltas).await;
}

/// Resolve an active machine build's recorded log file by its derivation path.
///
/// This is the gate on `/api/global-log`: the server only ever opens paths the
/// status directory itself advertised for a *currently active* build, so the
/// endpoint cannot be steered at arbitrary files.
pub async fn log_file_for(monitor: &Arc<RwLock<MonitorState>>, drv_path: &str) -> Option<PathBuf> {
    monitor
        .read()
        .await
        .global
        .builds
        .iter()
        .find(|build| build.drv_path.as_deref() == Some(drv_path))
        .and_then(|build| build.log_file.as_deref().map(PathBuf::from))
}

/// Read the tail of one build's on-disk log, decompressing when needed.
///
/// Nix compresses build logs *while writing them* (`.drv.bz2` under
/// `/nix/var/log/nix/drvs`), so a live log is a truncated bzip2 stream. `nix
/// log` refuses such a stream, which is why the server reads the file itself:
/// [`decompress_prefix`] keeps everything decoded before the truncation point.
/// The blocking read and decompression run off the async executor.
///
/// # Errors
///
/// Returns the underlying I/O error when the file cannot be read (most
/// commonly [`std::io::ErrorKind::NotFound`] before the builder's first output
/// flush creates it).
pub async fn read_log_tail(path: PathBuf) -> std::io::Result<String> {
    tokio::task::spawn_blocking(move || {
        let bytes = std::fs::read(&path)?;
        let decoded = if path.extension().is_some_and(|extension| extension == "bz2") {
            decompress_prefix(&bytes)
        } else {
            DecodedTail {
                bytes,
                prefix_dropped: false,
            }
        };
        Ok(tail_lines(
            &decoded.bytes,
            LOG_TAIL_BYTES,
            decoded.prefix_dropped,
        ))
    })
    .await
    // The closure neither panics nor is cancelled; a join error here is a bug.
    .unwrap_or_else(|join_error| {
        Err(std::io::Error::other(format!(
            "log read task failed: {join_error}"
        )))
    })
}

/// Decompress as much of a bzip2 stream as is decodable, bounded to a rolling
/// tail. A log being written is truncated mid-block and has no stream footer,
/// so a decode error just ends the read: everything decoded so far *is* the
/// live log. Only the last [`LOG_TAIL_BYTES`]-ish bytes are retained while
/// decoding, so a huge log never balloons memory.
///
/// bzip2 is a block format (the BWT inverse needs the whole block), so the
/// live tail advances one *completed* block at a time: nix compresses at the
/// default 900 KB block size, meaning a quiet build's log decodes to nothing
/// until its first 900 KB of output. That granularity is inherent to reading
/// what nix wrote; the panel shows "no log output yet" until then.
/// What the tolerant decode retained: the newest decoded bytes, plus whether
/// the rolling cap discarded earlier bytes (so the buffer no longer starts at
/// the log's true beginning and the caller must cut to a line boundary).
struct DecodedTail {
    bytes: Vec<u8>,
    prefix_dropped: bool,
}

fn decompress_prefix(bytes: &[u8]) -> DecodedTail {
    let mut decoder = bzip2::read::BzDecoder::new(bytes);
    let mut out = Vec::new();
    let mut dropped = false;
    let mut chunk = vec![0_u8; 64 * 1024];
    loop {
        match decoder.read(&mut chunk) {
            Ok(read) if read > 0 => {
                out.extend_from_slice(&chunk[..read]);
                if out.len() > LOG_TAIL_BYTES * 2 {
                    out.drain(..out.len() - LOG_TAIL_BYTES);
                    dropped = true;
                }
            }
            // Clean EOF, or a truncated live stream / garbage: either way,
            // everything decoded so far is the readable log.
            Ok(_) | Err(_) => break,
        }
    }
    DecodedTail {
        bytes: out,
        prefix_dropped: dropped,
    }
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

    /// A complete `.bz2` log round-trips; the tail keeps the end of the log.
    #[test]
    fn bz2_log_round_trips_and_tails() {
        let text = "configuring\nbuilding\ninstalling\n";
        let decoded = decompress_prefix(&compress_bzip2(text, 9));
        assert_eq!(decoded.bytes, text.as_bytes());
        assert!(!decoded.prefix_dropped, "a small log keeps its whole prefix");
        assert_eq!(tail_lines(text.as_bytes(), 1 << 20, false), text);
    }

    /// A log truncated mid-stream (the live-write case: no bzip2 footer, last
    /// block cut short) still yields every *completed* block instead of an
    /// error. This is the exact case `nix log` refuses and the reason the
    /// server decompresses the file itself. Level 1 (100 KB blocks) keeps the
    /// fixture small while spanning several blocks; nix's level-9 stream
    /// behaves identically at 900 KB granularity.
    #[test]
    fn truncated_bz2_stream_yields_completed_blocks() {
        let line = "log line with some incompressible entropy 8d1f4a2c\n";
        // ~500 KB uncompressed -> ~5 level-1 blocks, and enough decoded output
        // to exercise the rolling cap.
        let text = line.repeat(10_000);
        let compressed = compress_bzip2(&text, 1);
        let truncated = &compressed[..compressed.len() / 2];

        let decoded = decompress_prefix(truncated);
        assert!(!decoded.bytes.is_empty(), "completed blocks decode");
        assert!(decoded.bytes.len() < text.len(), "but not the whole log");
        assert!(
            String::from_utf8_lossy(&decoded.bytes).contains("incompressible entropy 8d1f4a2c"),
            "decoded bytes are real log content"
        );
    }

    /// A truncated stream whose first block never completed (a quiet build's
    /// live log) decodes to nothing, and must not error: the panel shows
    /// "no log output yet".
    #[test]
    fn truncated_first_block_decodes_to_empty() {
        let compressed = compress_bzip2(&"short log\n".repeat(100), 9);
        let truncated = &compressed[..compressed.len() / 2];
        assert!(decompress_prefix(truncated).bytes.is_empty());
    }

    /// Garbage that is not bzip2 at all decodes to nothing rather than failing.
    #[test]
    fn non_bzip2_bytes_decode_to_empty() {
        assert!(decompress_prefix(b"error: not a log").bytes.is_empty());
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
    /// the drv must be active *and* carry a recorded log. This is the
    /// arbitrary-file-read gate on `/api/global-log`.
    #[tokio::test]
    async fn log_file_for_resolves_only_active_builds() {
        let with_log = GlobalBuild {
            drv_path: Some("/nix/store/aaa-foo.drv".to_owned()),
            log_file: Some("/nix/var/log/nix/drvs/ab/cdfoo.drv.bz2".to_owned()),
            ..GlobalBuild::default()
        };
        let without_log = GlobalBuild {
            drv_path: Some("/nix/store/bbb-bar.drv".to_owned()),
            ..GlobalBuild::default()
        };
        let mut state = MonitorState::default();
        state.set_global(GlobalBuilds {
            detected: true,
            builds: vec![with_log, without_log],
            status: "2 active".to_owned(),
        });
        let monitor = Arc::new(RwLock::new(state));

        assert_eq!(
            log_file_for(&monitor, "/nix/store/aaa-foo.drv").await,
            Some(PathBuf::from("/nix/var/log/nix/drvs/ab/cdfoo.drv.bz2"))
        );
        assert_eq!(log_file_for(&monitor, "/nix/store/bbb-bar.drv").await, None);
        assert_eq!(log_file_for(&monitor, "/etc/passwd").await, None);
    }

    /// Reading a real compressed file end-to-end through the async entry point.
    #[tokio::test]
    async fn read_log_tail_reads_compressed_file() {
        let dir = std::env::temp_dir().join(format!("nwm-global-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        let path = dir.join("test.drv.bz2");
        std::fs::write(&path, compress_bzip2("hello from the builder\n", 9))
            .expect("write fixture log");

        let tail = read_log_tail(path).await.expect("fixture log reads");
        assert_eq!(tail, "hello from the builder\n");

        std::fs::remove_dir_all(&dir).expect("clean scratch dir");
    }

    /// A missing log file (builder has not flushed yet) is a clean `NotFound`,
    /// which the endpoint maps to 404 rather than an empty 200.
    #[tokio::test]
    async fn read_log_tail_missing_file_is_not_found() {
        let error = read_log_tail(PathBuf::from("/nonexistent/nwm-test.drv.bz2"))
            .await
            .expect_err("missing file errors");
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }
}
