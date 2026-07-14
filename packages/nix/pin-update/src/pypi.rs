//! `PyPI` sdist pin refresher for a `pins.json` of Python source pins.
//!
//! Policy markers on an entry, honored here so pins are skipped or narrowed
//! loudly instead of guessed at:
//!
//! - `prefetch = "manual"`: hash-mode hold for platform-specific artifacts
//!   whose URL/hash must be refreshed by hand.
//! - `hold = "<reason>"`: version hold for pins whose dependency override set
//!   is hand-tuned to one exact upstream release.
//! - `track = "<dotted prefix>"`: follow only the newest release inside that
//!   version line.

use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use indexmap::IndexMap;
use serde::Deserialize;

use crate::pins_file::{self, Entry};
use crate::{http, sri};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    /// Repo-relative pins.json of `PyPI` sdist pins to rewrite.
    pins: PathBuf,
}

pub fn run(spec: &Spec) -> Result<()> {
    let mut pins = pins_file::read(&spec.pins)?;
    for (name, entry) in &mut pins {
        refresh_pin(name, entry)?;
    }
    pins_file::write(&spec.pins, &pins)?;
    println!("updated {} from PyPI metadata", spec.pins.display());
    Ok(())
}

#[derive(Deserialize)]
struct Metadata {
    info: Info,
    releases: IndexMap<String, Vec<Artifact>>,
}

#[derive(Deserialize)]
struct Info {
    version: String,
}

#[derive(Deserialize)]
struct Artifact {
    packagetype: String,
    filename: String,
    digests: Digests,
}

#[derive(Deserialize)]
struct Digests {
    sha256: String,
}

fn refresh_pin(name: &str, entry: &mut Entry) -> Result<()> {
    if let Some(prefetch) = pins_file::str_field(entry, "prefetch") {
        if prefetch != "manual" {
            bail!(
                "{name}: unsupported prefetch policy {prefetch}; this updater only handles flat PyPI sdist pins and prefetch=manual holds"
            );
        }
        println!("skipping {name}: prefetch=manual; refresh this platform pin by hand");
        return Ok(());
    }
    if let Some(hold) = pins_file::str_field(entry, "hold") {
        println!("skipping {name}: hold={hold}");
        return Ok(());
    }

    let metadata: Metadata = http::get_json(&format!("https://pypi.org/pypi/{name}/json"))
        .with_context(|| format!("{name}: failed to fetch PyPI metadata"))?;
    let version = match pins_file::str_field(entry, "track") {
        Some(track) => tracked_version(name, metadata.releases.keys(), track)?,
        None => metadata.info.version,
    };

    let release = metadata
        .releases
        .get(&version)
        .with_context(|| format!("{name}: PyPI metadata lists no release {version}"))?;
    let Some(sdist) = release
        .iter()
        .find(|artifact| artifact.packagetype == "sdist")
    else {
        bail!("{name}: PyPI release {version} has no sdist");
    };

    let url = source_url(name, &sdist.filename)?;
    let hash = sri::from_hex(&sdist.digests.sha256)?;
    pins_file::set_str(entry, "version", version);
    pins_file::set_str(entry, "url", url);
    pins_file::set_str(entry, "hash", hash);
    Ok(())
}

/// The stable `pypi.io` source redirect for a project's sdist filename.
fn source_url(project: &str, filename: &str) -> Result<String> {
    let first = project
        .chars()
        .next()
        .context("empty PyPI project name")?;
    Ok(format!(
        "https://pypi.io/packages/source/{first}/{project}/{filename}"
    ))
}

/// A pin candidate is a plain dotted-integer version. `PyPI` release lists also
/// carry pre-releases (3.5.6.dev1, 4.0.0rc1); those never become pins, so
/// they are disqualified here rather than failing an integer parse.
fn version_segments(version: &str) -> Option<Vec<u64>> {
    version
        .split('.')
        .map(|segment| segment.parse().ok())
        .collect()
}

/// Newest release whose dotted-integer segments start with `track`'s.
fn tracked_version<'a>(
    name: &str,
    releases: impl Iterator<Item = &'a String>,
    track: &str,
) -> Result<String> {
    let track_segments = version_segments(track)
        .with_context(|| format!("{name}: track {track} is not a dotted-integer prefix"))?;
    let newest = releases
        .filter(|version| {
            version_segments(version).is_some_and(|segments| segments.starts_with(&track_segments))
        })
        .max_by_key(|version| version_segments(version));
    newest
        .cloned()
        .with_context(|| format!("{name}: no PyPI releases match track {track}"))
}

#[cfg(test)]
mod tests {
    use super::{source_url, tracked_version, version_segments};

    #[test]
    fn pre_releases_are_not_version_candidates() {
        assert_eq!(version_segments("3.5.6"), Some(vec![3, 5, 6]));
        assert_eq!(version_segments("3.5.6.dev1"), None);
        assert_eq!(version_segments("4.0.0rc1"), None);
    }

    #[test]
    fn track_selects_the_newest_release_in_its_line() {
        let releases: Vec<String> = ["3.5.4", "3.5.5", "3.6.0", "4.0.0.dev2", "3.5.10"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        let version = tracked_version("pyspark", releases.iter(), "3.5").unwrap();
        assert_eq!(version, "3.5.10");
    }

    #[test]
    fn track_with_no_matching_release_fails() {
        let releases: Vec<String> = vec!["2.0.0".to_owned()];
        let err = tracked_version("asn1", releases.iter(), "3").unwrap_err();
        assert!(err.to_string().contains("no PyPI releases match track 3"));
    }

    #[test]
    fn sdist_urls_pin_through_the_source_redirect() {
        assert_eq!(
            source_url("htpy", "htpy-26.5.1.tar.gz").unwrap(),
            "https://pypi.io/packages/source/h/htpy/htpy-26.5.1.tar.gz"
        );
    }
}
