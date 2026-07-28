//! `complexity.toml`: the committed thresholds and the ratchet.

use std::{collections::BTreeMap, path::Path};

use snafu::{ResultExt as _, Snafu};

pub const FILENAME: &str = "complexity.toml";

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Glob patterns excluded before anything is measured. Applied to the
    /// denominator as well as the numerator: an ignore that does not move the
    /// gated number is not an ignore.
    #[serde(default)]
    pub ignore: Vec<String>,
    /// Cognitive threshold per language name, from `complexity quantiles`.
    #[serde(default)]
    pub threshold: BTreeMap<String, u32>,
    #[serde(default)]
    pub budget: Budget,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    /// How many units may sit at or above their language's threshold. The
    /// ratchet: lower it as units are broken down, never raise it silently.
    pub max_over_threshold: Option<usize>,
}

#[derive(Debug, Snafu)]
// clone:ignore -- identifier-blind shape match with ast-merge-git's unrelated
// RevisionError: two snafu enums whose variants both carry `{path, source}`
// normalize to the same tree. Merging them would couple config loading to git
// revision I/O to satisfy a structural detector, which is the wrong trade.
pub enum Error {
    #[snafu(display("failed to read {path}"))]
    Read {
        path: String,
        source: std::io::Error,
    },
    #[snafu(display("failed to parse {path}"))]
    Parse {
        path: String,
        source: toml::de::Error,
    },
}

/// Load the config found by walking up from `start`, or the default when no
/// file exists anywhere above it.
///
/// # Errors
/// Returns an error if a file is found but cannot be read or parsed.
pub fn load(start: &Path) -> Result<Config, Error> {
    let Some(path) = find(start) else {
        return Ok(Config::default());
    };
    let display = path.display().to_string();
    let text = std::fs::read_to_string(&path).context(ReadSnafu { path: &display })?;
    toml::from_str(&text).context(ParseSnafu { path: display })
}

fn find(start: &Path) -> Option<std::path::PathBuf> {
    let mut dir = if start.is_dir() {
        start.to_path_buf()
    } else {
        start.parent()?.to_path_buf()
    };
    loop {
        let candidate = dir.join(FILENAME);
        if candidate.is_file() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Config, load};

    #[test]
    fn a_missing_file_is_the_default_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = load(dir.path()).expect("default config");
        assert!(config.threshold.is_empty());
        assert_eq!(config.budget.max_over_threshold, None);
    }

    #[test]
    fn parses_thresholds_and_budget() {
        let config: Config = toml::from_str(
            r#"
ignore = ["*/target/*"]

[threshold]
Rust = 14

[budget]
max_over_threshold = 7
"#,
        )
        .expect("parses");
        assert_eq!(config.threshold.get("Rust"), Some(&14));
        assert_eq!(config.budget.max_over_threshold, Some(7));
        assert_eq!(config.ignore, vec!["*/target/*".to_owned()]);
    }

    /// A typo in a threshold table silently disables a gate, so an unknown key
    /// has to fail loudly instead.
    #[test]
    fn rejects_unknown_keys() {
        assert!(toml::from_str::<Config>("budget_pct = 1.0").is_err());
    }

    #[test]
    fn walks_up_to_find_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("complexity.toml"), "[threshold]\nNix = 3\n")
            .expect("write");
        let nested = dir.path().join("a/b");
        std::fs::create_dir_all(&nested).expect("mkdir");
        let config = load(&nested).expect("found");
        assert_eq!(config.threshold.get("Nix"), Some(&3));
    }
}
