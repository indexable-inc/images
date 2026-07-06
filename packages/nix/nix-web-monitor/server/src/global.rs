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

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use nix_web_monitor_parser::{GlobalBuilds, MonitorState};
use nix_web_monitor_parser::global::parse_builds;
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
/// "unknown command" / "unrecognised flag" error to stderr (not a JSON array),
/// so every variant fails to parse and this returns `None` -> undetected.
async fn poll_builds() -> Option<Vec<nix_web_monitor_parser::GlobalBuild>> {
    // The patched command may be gated behind an experimental feature, but an
    // *unknown* experimental feature is itself an error on stock nix. So try the
    // plain form first (works if the command is ungated) and, only if that does
    // not parse, the feature-gated form. Whichever yields a JSON array is used;
    // if neither does, the subcommand is not available.
    const ATTEMPTS: [&[&str]; 2] = [
        &["store", "builds", "--json", "--extra-experimental-features", "nix-command"],
        &[
            "store",
            "builds",
            "--json",
            "--extra-experimental-features",
            "nix-command build-status-dir",
        ],
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
async fn try_builds(args: &[&str]) -> Option<Vec<nix_web_monitor_parser::GlobalBuild>> {
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

#[cfg(test)]
mod tests {
    use nix_web_monitor_parser::{GlobalBuildKind, MonitorState};

    use super::*;

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
}
