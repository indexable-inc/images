use std::sync::Arc;

use anyhow::{Context, Result, bail};
use nix_web_monitor_parser::MonitorState;
use tokio::process::Command;
use tokio::sync::{RwLock, broadcast, mpsc};

use crate::publish_snapshot;

/// Suffix Nix uses for derivation files. A built derivation also references
/// source paths; only the `.drv` references are edges in the build DAG.
const DRV_SUFFIX: &str = ".drv";

/// Resolve dependency edges for built derivations out-of-band.
///
/// The internal-json stream names each derivation Nix builds but carries no
/// edges between them. For every derivation reported, we ask `nix-store
/// --query --references` for its direct inputs and feed them back into the
/// monitor, where [`MonitorState::snapshot`] turns the adjacency into the
/// rendered DAG. Runs as its own task so the per-derivation process spawn never
/// stalls stderr parsing. The task exits when `derivations` closes, which the
/// stderr reader does by dropping its sender once Nix's stderr ends.
pub async fn resolve_dependencies(
    mut derivations: mpsc::UnboundedReceiver<String>,
    monitor: Arc<RwLock<MonitorState>>,
    snapshots: broadcast::Sender<String>,
) -> Result<()> {
    while let Some(derivation) = derivations.recv().await {
        let inputs = match direct_input_derivations(&derivation).await {
            Ok(inputs) => inputs,
            // A derivation whose references cannot be queried simply has no
            // edges. Surface it on stderr and keep resolving the rest rather
            // than tearing down the whole graph for one missing node.
            Err(error) => {
                eprintln!("nix-web-monitor: dependency query failed for {derivation}: {error:#}");
                continue;
            }
        };

        monitor
            .write()
            .await
            .record_direct_dependencies(derivation, inputs);
        publish_snapshot(&monitor, &snapshots).await?;
    }
    Ok(())
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
