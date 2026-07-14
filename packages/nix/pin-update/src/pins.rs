//! Generic `pins.json` re-pinner: refresh each entry's SRI `hash` from its
//! pinned `url`, preserving every other field and the file's entry order.
//!
//! The JSON contract (required `hash`, optional `url`/`rev`/`version`
//! coordinates, `prefetch` mode) is owned by `lib/util/pins.nix`, whose
//! `loadPins` validates it at eval; this mode is the mechanical rewrite
//! half. `prefetch` selects how the fetcher's hash is recomputed:
//!
//! - `file` (default): flat file hash via `nix store prefetch-file`, matching
//!   `fetchurl`.
//! - `unpack`: unpacked-tree hash via `nix-prefetch-url --unpack`, matching
//!   `fetchzip` (default `stripRoot = true`) and `fetchCrate`.
//! - `manual`: never rewritten; no prefetch command reproduces the hash
//!   (e.g. `fetchzip { stripRoot = false; }`), so a human refreshes it from
//!   the build's `got:` mismatch error.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, bail};
use serde::Deserialize;

use crate::pins_file::{self, Entry};
use crate::{cmd, sri};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    /// Repo-relative pins.json to rewrite.
    pins: PathBuf,
}

pub fn run(spec: &Spec) -> Result<()> {
    let mut pins = pins_file::read(&spec.pins)?;
    for (name, entry) in &mut pins {
        refresh_entry(&spec.pins, name, entry)?;
    }
    pins_file::write(&spec.pins, &pins)?;
    println!("re-pinned {}", spec.pins.display());
    Ok(())
}

fn refresh_entry(path: &Path, name: &str, entry: &mut Entry) -> Result<()> {
    let mode = pins_file::str_field(entry, "prefetch")
        .unwrap_or("file")
        .to_owned();
    if mode == "manual" {
        println!(
            "skipping {name}: prefetch=manual; refresh by building with the new url and copying the got: hash"
        );
        return Ok(());
    }
    let Some(url) = pins_file::str_field(entry, "url").map(str::to_owned) else {
        println!("skipping {name}: no `url` to re-fetch");
        return Ok(());
    };

    let hash = match mode.as_str() {
        // fetchzip/fetchCrate validate the UNPACKED tree, not the archive
        // bytes; `nix-prefetch-url --unpack` reproduces that hash
        // (fetchTarball semantics: single root dir stripped).
        "unpack" => {
            let base32 = cmd::stdout(Command::new("nix-prefetch-url").args(["--unpack", &url]))?;
            sri::from_nix_base32(&base32)?
        }
        "file" => prefetch_file(&url)?,
        other => bail!(
            "{}: pin {name} has unknown prefetch mode {other}; expected file, unpack, or manual",
            path.display()
        ),
    };
    pins_file::set_str(entry, "hash", hash);
    Ok(())
}

fn prefetch_file(url: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct Prefetched {
        hash: String,
    }

    let raw = cmd::stdout(
        Command::new("nix")
            .args(["store", "prefetch-file", "--json"])
            .arg(url),
    )?;
    let prefetched: Prefetched = serde_json::from_str(&raw)
        .with_context(|| format!("unexpected `nix store prefetch-file --json` output for {url}"))?;
    Ok(prefetched.hash)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::pins_file::Pins;

    use super::refresh_entry;

    fn pins(json: &str) -> Pins {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn manual_and_urlless_pins_are_left_untouched() {
        let mut doc = pins(
            r#"{
              "held": {"url": "https://example.com/a", "hash": "sha256-x", "prefetch": "manual"},
              "bare": {"hash": "sha256-y"}
            }"#,
        );
        let before = doc.clone();
        for (name, entry) in &mut doc {
            refresh_entry(Path::new("pins.json"), name, entry).unwrap();
        }
        assert_eq!(doc, before);
    }

    #[test]
    fn unknown_prefetch_mode_fails_loudly() {
        let mut doc =
            pins(r#"{"p": {"url": "https://example.com", "hash": "h", "prefetch": "zip"}}"#);
        let (name, entry) = doc.iter_mut().next().unwrap();
        let err = refresh_entry(Path::new("pins.json"), name, entry).unwrap_err();
        assert!(err.to_string().contains("unknown prefetch mode zip"));
    }

    #[test]
    fn entry_order_survives_a_round_trip() {
        let text = r#"{
  "b": {
    "version": "1",
    "url": "https://example.com",
    "hash": "sha256-x"
  },
  "a": {
    "hash": "sha256-y"
  }
}"#;
        let doc: Pins = serde_json::from_str(text).unwrap();
        let mut out = serde_json::to_string_pretty(&doc).unwrap();
        out.push('\n');
        assert_eq!(out, format!("{text}\n"));
    }
}
