//! Node specs: which `darwinConfigurations` attribute goes to which ssh
//! destination.

use std::str::FromStr;

use anyhow::{Context, Result, bail};

/// An ssh destination, kept as typed fields so the ssh argv and the
/// `ssh://` store URL are rendered from the same facts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub user: Option<String>,
    pub host: String,
}

impl Target {
    /// The `[user@]host` token ssh and `ssh://` store URLs share.
    pub fn ssh_destination(&self) -> String {
        self.user
            .as_ref()
            .map_or_else(|| self.host.clone(), |user| format!("{user}@{}", self.host))
    }

    /// The store URL `nix copy --to` pushes through.
    pub fn store_url(&self) -> String {
        format!("ssh://{}", self.ssh_destination())
    }

    /// Whether remote commands already run as root (no sudo prefix needed).
    pub fn is_root(&self) -> bool {
        self.user.as_deref() == Some("root")
    }
}

impl FromStr for Target {
    type Err = anyhow::Error;

    fn from_str(destination: &str) -> Result<Self> {
        let (user, host) = destination
            .split_once('@')
            .map_or((None, destination), |(user, host)| (Some(user), host));
        if let Some(user) = user
            && user.is_empty()
        {
            bail!("destination `{destination}` has an empty user");
        }
        if host.is_empty() {
            bail!("destination `{destination}` has an empty host");
        }
        Ok(Self {
            user: user.map(str::to_owned),
            host: host.to_owned(),
        })
    }
}

/// One deployment target: `name` indexes `darwinConfigurations` in the flake,
/// `target` is where the closure goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeSpec {
    pub name: String,
    pub target: Target,
}

impl FromStr for NodeSpec {
    type Err = anyhow::Error;

    fn from_str(spec: &str) -> Result<Self> {
        let (name, destination) = spec
            .split_once('=')
            .with_context(|| format!("node `{spec}` is not of the form `<name>=<[user@]host>`"))?;
        if name.is_empty() {
            bail!("node `{spec}` has an empty configuration name");
        }
        let target = destination
            .parse()
            .with_context(|| format!("node `{spec}` has an invalid destination"))?;
        Ok(Self {
            name: name.to_owned(),
            target,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_name_user_and_host() {
        let spec: NodeSpec = "mac1=admin@192.168.64.6".parse().expect("parses");
        assert_eq!(spec.name, "mac1");
        assert_eq!(spec.target.user.as_deref(), Some("admin"));
        assert_eq!(spec.target.host, "192.168.64.6");
        assert_eq!(spec.target.ssh_destination(), "admin@192.168.64.6");
        assert_eq!(spec.target.store_url(), "ssh://admin@192.168.64.6");
        assert!(!spec.target.is_root());
    }

    #[test]
    fn parses_bare_host() {
        let spec: NodeSpec = "mac1=mac1.local".parse().expect("parses");
        assert_eq!(spec.target.user, None);
        assert_eq!(spec.target.ssh_destination(), "mac1.local");
        assert_eq!(spec.target.store_url(), "ssh://mac1.local");
    }

    #[test]
    fn recognizes_root_user() {
        let spec: NodeSpec = "mac1=root@mac1.local".parse().expect("parses");
        assert!(spec.target.is_root());
    }

    #[test]
    fn rejects_malformed_specs() {
        for spec in ["mac1", "=host", "mac1=", "mac1=@host", "mac1=user@"] {
            assert!(
                spec.parse::<NodeSpec>().is_err(),
                "`{spec}` should be rejected"
            );
        }
    }
}
