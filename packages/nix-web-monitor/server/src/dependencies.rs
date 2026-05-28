use std::collections::BTreeSet;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use bytes::Bytes;
use nix_web_monitor_parser::MonitorState;
use tokio::process::Command;
use tokio::sync::{RwLock, Semaphore, broadcast, mpsc};
use tokio::task::JoinSet;

use crate::broadcast_deltas;

/// Suffix Nix uses for derivation files. A derivation's closure also contains
/// source paths; only the `.drv` entries are nodes in the build DAG.
const DRV_SUFFIX: &str = ".drv";

/// Concurrent `nix-store --query --requisites` processes. A large `nix build`
/// reports hundreds of derivations; querying them one at a time serialises that
/// many process spawns and the edges trickle into the tree long after the
/// builds are visible. The cap keeps the process fan-out bounded while still
/// filling the DAG quickly.
const MAX_CONCURRENT_QUERIES: usize = 16;

/// Resolve dependency edges for rendered derivations out-of-band.
///
/// The internal-json stream names each derivation Nix builds but carries no
/// edges between them. For every derivation reported, we ask `nix-store
/// --query --requisites` for its full transitive `.drv` closure and feed it
/// back into the monitor, where [`MonitorState::snapshot`] reduces the closures
/// into the rendered DAG. Querying the transitive closure (not just direct
/// references) is what lets the DAG bridge through cached intermediates Nix
/// never reports, so a deep dependency stays nested under its target. Queries
/// run concurrently (bounded by [`MAX_CONCURRENT_QUERIES`]) so the tree fills in
/// while builds are still in flight rather than after. Runs as its own task so
/// the per-derivation process spawns never stall stderr parsing. The task exits
/// once `derivations` closes (the stderr reader drops its sender when Nix's
/// stderr ends) and the in-flight queries drain.
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

/// Query one derivation's transitive `.drv` closure and fold it into the
/// monitor. A query failure is non-fatal: the derivation simply contributes no
/// edges, so it is logged and swallowed rather than tearing down the resolver.
async fn resolve_one(
    derivation: String,
    monitor: &Arc<RwLock<MonitorState>>,
    deltas: &broadcast::Sender<Bytes>,
) -> Result<()> {
    let closure = match closure_derivations(&derivation).await {
        Ok(closure) => closure,
        Err(error) => {
            eprintln!("nix-web-monitor: dependency query failed for {derivation}: {error:#}");
            return Ok(());
        }
    };

    monitor.write().await.record_closure(derivation, closure);
    broadcast_deltas(monitor, deltas).await
}

/// Transitive input derivations of `derivation`, via the decade-stable
/// `nix-store --query --requisites` interface (plain newline-separated store
/// paths, unlike the version-dependent `nix derivation show` JSON). The closure
/// includes `derivation` itself, which is filtered out so it is not recorded as
/// its own dependency.
async fn closure_derivations(derivation: &str) -> Result<BTreeSet<String>> {
    let output = Command::new("nix-store")
        .arg("--query")
        .arg("--requisites")
        .arg(derivation)
        .output()
        .await
        .context("spawning nix-store --query --requisites")?;

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
        .filter(|path| path.ends_with(DRV_SUFFIX) && *path != derivation)
        .map(ToOwned::to_owned)
        .collect())
}
