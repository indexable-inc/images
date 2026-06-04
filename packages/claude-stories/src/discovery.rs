//! Peer discovery. The default transport is the Tailscale tailnet: `tailscale
//! status --json` is already an authenticated, NAT-traversed list of every
//! online device, so it doubles as the peer directory. No central server, no
//! DHT, no bootstrap nodes. `CLAUDE_STORIES_PEERS` overrides it with an explicit
//! list for testing or off-tailnet use.

use color_eyre::Result;
use color_eyre::eyre::{Context, eyre};
use serde::Deserialize;

/// Where to find peers.
pub enum Discovery {
    /// Static `host:port` (or full URL) list from `CLAUDE_STORIES_PEERS`.
    Peers(Vec<String>),
    /// Every online peer on the tailnet.
    Tailnet,
}

impl Discovery {
    /// Choose a transport from the environment: an explicit
    /// `CLAUDE_STORIES_PEERS` list if set, otherwise the tailnet. Any failure to
    /// actually reach peers surfaces later, as a typed error from
    /// [`Self::endpoints`].
    #[must_use]
    pub fn from_env() -> Self {
        if let Some(list) = std::env::var_os("CLAUDE_STORIES_PEERS") {
            let peers: Vec<String> = list
                .to_string_lossy()
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect();
            return Self::Peers(peers);
        }
        Self::Tailnet
    }

    /// Build the `/story` endpoint URLs to fetch, one per peer.
    pub fn endpoints(&self, port: u16) -> Result<Vec<String>> {
        match self {
            Self::Peers(peers) => Ok(peers.iter().map(|p| story_url(p, port)).collect()),
            Self::Tailnet => tailnet_endpoints(port),
        }
    }
}

/// Normalize a peer entry into a full `/story` URL.
fn story_url(peer: &str, port: u16) -> String {
    let base = if peer.starts_with("http://") || peer.starts_with("https://") {
        peer.trim_end_matches('/').to_owned()
    } else if peer.contains(':') {
        format!("http://{peer}")
    } else {
        format!("http://{peer}:{port}")
    };
    if base.ends_with("/story") { base } else { format!("{base}/story") }
}

// Subset of `tailscale status --json` we need.
#[derive(Deserialize)]
struct TsStatus {
    #[serde(rename = "Peer", default)]
    peer: std::collections::HashMap<String, TsPeer>,
}

#[derive(Deserialize)]
struct TsPeer {
    #[serde(rename = "TailscaleIPs", default)]
    ips: Vec<String>,
    #[serde(rename = "Online", default)]
    online: bool,
}

fn tailnet_endpoints(port: u16) -> Result<Vec<String>> {
    let out = std::process::Command::new("tailscale")
        .args(["status", "--json"])
        .output()
        .wrap_err("running `tailscale status --json` (is the Tailscale CLI installed?)")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(eyre!("tailscale status failed: {}", stderr.trim()));
    }
    let status: TsStatus =
        serde_json::from_slice(&out.stdout).wrap_err("parsing tailscale status JSON")?;

    let mut endpoints = Vec::new();
    for peer in status.peer.values() {
        if !peer.online {
            continue;
        }
        // Prefer the IPv4 (100.x) address; it is the most broadly reachable.
        if let Some(ip) = peer
            .ips
            .iter()
            .find(|ip| ip.parse::<std::net::Ipv4Addr>().is_ok())
        {
            endpoints.push(format!("http://{ip}:{port}/story"));
        }
    }
    Ok(endpoints)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn story_url_normalizes() {
        assert_eq!(story_url("host", 4810), "http://host:4810/story");
        assert_eq!(story_url("100.1.2.3:9000", 4810), "http://100.1.2.3:9000/story");
        assert_eq!(story_url("http://x:1/story", 4810), "http://x:1/story");
        assert_eq!(story_url("https://x/", 4810), "https://x/story");
    }
}
