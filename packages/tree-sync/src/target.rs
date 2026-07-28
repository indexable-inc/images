//! Where a tree is being synced to.

use std::path::{Path, PathBuf};

use color_eyre::Result;
use color_eyre::eyre::eyre;

/// A sync destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A directory on this machine: another checkout, or a worktree.
    Local {
        /// The destination directory.
        path: PathBuf,
    },
    /// A directory on `host`, reached over ssh.
    Remote {
        /// Anything ssh accepts, including `user@host` and a `~/.ssh/config`
        /// alias.
        host: String,
        /// The destination directory on that host.
        path: PathBuf,
    },
}

impl Target {
    /// Parse a destination argument.
    ///
    /// `host:/path` and `host:path` are remote, matching scp and rsync. A
    /// spec with no colon, or whose pre-colon part looks like a path, is local.
    ///
    /// # Errors
    /// Returns an error for `host:` with an empty path. rsync reads that as the
    /// remote home directory, which makes a mistyped destination combined with
    /// `--delete` a very bad afternoon; refusing is cheaper than guessing.
    pub fn parse(spec: &str) -> Result<Self> {
        let Some((host, path)) = spec.split_once(':') else {
            return Ok(Self::Local {
                path: PathBuf::from(spec),
            });
        };

        // A colon inside something that is already a path (`./a:b`, `/a:b`) is
        // part of the filename, not a host separator.
        if host.is_empty() || host.contains('/') {
            return Ok(Self::Local {
                path: PathBuf::from(spec),
            });
        }

        if path.is_empty() {
            return Err(eyre!(
                "destination {spec:?} names a host with no path; write \
                 {host}:/absolute/path or {host}:relative/path"
            ));
        }

        Ok(Self::Remote {
            host: host.to_owned(),
            path: PathBuf::from(path),
        })
    }

    /// The destination directory, without the host.
    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            Self::Local { path } | Self::Remote { path, .. } => path,
        }
    }

    /// How the destination should read back in the run summary.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Local { path } => path.display().to_string(),
            Self::Remote { host, path } => format!("{host}:{}", path.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Target;
    use std::path::PathBuf;

    #[test]
    fn remote_specs_split_on_the_first_colon() {
        assert_eq!(
            Target::parse("dc3:/tmp/ix").expect("parses"),
            Target::Remote {
                host: "dc3".to_owned(),
                path: PathBuf::from("/tmp/ix"),
            }
        );
        assert_eq!(
            Target::parse("root@10.0.0.4:work/ix").expect("parses"),
            Target::Remote {
                host: "root@10.0.0.4".to_owned(),
                path: PathBuf::from("work/ix"),
            }
        );
    }

    #[test]
    fn paths_stay_local_even_with_a_colon_in_them() {
        assert_eq!(
            Target::parse("/tmp/other-checkout").expect("parses"),
            Target::Local {
                path: PathBuf::from("/tmp/other-checkout"),
            }
        );
        assert_eq!(
            Target::parse("./notes/2026:07:27").expect("parses"),
            Target::Local {
                path: PathBuf::from("./notes/2026:07:27"),
            }
        );
    }

    #[test]
    fn a_host_with_no_path_is_refused_rather_than_guessed_at() {
        let error = Target::parse("dc3:").expect_err("empty remote path is refused");
        assert!(
            error.to_string().contains("names a host with no path"),
            "unexpected error: {error}"
        );
    }
}
