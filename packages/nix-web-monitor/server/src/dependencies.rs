use std::sync::Arc;

use anyhow::{Context, Result, bail};
use bytes::Bytes;
use nix_web_monitor_parser::MonitorState;
use tokio::process::Command;
use tokio::sync::{RwLock, Semaphore, broadcast, mpsc};
use tokio::task::JoinSet;

use crate::broadcast_deltas;

/// Suffix Nix uses for derivation files. A built derivation also references
/// source paths; only the `.drv` references are edges in the build DAG.
const DRV_SUFFIX: &str = ".drv";

/// Concurrent `nix-store --query --references` processes. A large `nix build`
/// reports hundreds of derivations; querying them one at a time serialises that
/// many process spawns and the edges trickle into the tree long after the
/// builds are visible. The cap keeps the process fan-out bounded while still
/// filling the DAG quickly.
const MAX_CONCURRENT_QUERIES: usize = 16;

/// Resolve dependency edges for built derivations out-of-band.
///
/// The internal-json stream names each derivation Nix builds but carries no
/// edges between them. For every derivation reported, we ask `nix-store
/// --query --references` for its direct inputs and feed them back into the
/// monitor, where [`MonitorState::snapshot`] turns the adjacency into the
/// rendered DAG. Queries run concurrently (bounded by [`MAX_CONCURRENT_QUERIES`])
/// so the tree fills in while builds are still in flight rather than after.
/// Runs as its own task so the per-derivation process spawns never stall stderr
/// parsing. The task exits once `derivations` closes (the stderr reader drops
/// its sender when Nix's stderr ends) and the in-flight queries drain.
pub async fn resolve_dependencies(
    mut derivations: mpsc::UnboundedReceiver<String>,
    monitor: Arc<RwLock<MonitorState>>,
    deltas: broadcast::Sender<Bytes>,
) -> Result<()> {
    let limit = Arc::new(Semaphore::new(MAX_CONCURRENT_QUERIES));
    let mut queries: JoinSet<Result<()>> = JoinSet::new();

    while let Some(derivation) = derivations.recv().await {
        // Acquire before spawning so an unbounded receive backlog cannot
        // outrun the process cap; the permit rides into the task and releases
        // when the query finishes.
        let permit = Arc::clone(&limit)
            .acquire_owned()
            .await
            .expect("dependency query semaphore is never closed");
        let monitor = Arc::clone(&monitor);
        let deltas = deltas.clone();
        queries.spawn(async move {
            let _permit = permit;
            resolve_one(derivation, &monitor, &deltas).await
        });

        // Reap finished queries opportunistically so the set does not grow for
        // the whole run and any serialization error surfaces promptly.
        while let Some(joined) = queries.try_join_next() {
            joined.context("joining dependency query task")??;
        }
    }

    while let Some(joined) = queries.join_next().await {
        joined.context("joining dependency query task")??;
    }
    Ok(())
}

/// Query one derivation's direct input `.drv`s and fold them into the monitor.
/// A query failure is non-fatal: the derivation simply contributes no edges, so
/// it is logged and swallowed rather than tearing down the whole resolver.
async fn resolve_one(
    derivation: String,
    monitor: &Arc<RwLock<MonitorState>>,
    deltas: &broadcast::Sender<Bytes>,
) -> Result<()> {
    let inputs = match direct_input_derivations(&derivation).await {
        Ok(inputs) => inputs,
        Err(error) => {
            eprintln!("nix-web-monitor: dependency query failed for {derivation}: {error:#}");
            return Ok(());
        }
    };

    monitor
        .write()
        .await
        .record_direct_dependencies(derivation, inputs);
    broadcast_deltas(monitor, deltas).await
}

/// Direct input derivations of `derivation`, via the decade-stable
/// `nix-store --query --references` interface (plain newline-separated store
/// paths, unlike the version-dependent `nix derivation show` JSON).
async fn direct_input_derivations(derivation: &str) -> Result<Vec<String>> {
    let output = Command::new("nix-store")
        .arg("--query")
        .arg("--references")
        .arg(derivation)
        .output()
        .await
        .context("spawning nix-store --query --references")?;

    if !output.status.success() {
        bail!(
            "nix-store exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|path| path.ends_with(DRV_SUFFIX))
        .map(ToOwned::to_owned)
        .collect())
}
