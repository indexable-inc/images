//! Claude Code updater: refresh manifest.json from Anthropic's published
//! per-version manifest, converting its hex checksums to the SRI hashes the
//! fetcher pins, then refresh the committed stock system-prompt snapshots.
//! Fails closed unless the manifest's detached GPG signature verifies
//! against the pinned release signing key, so a spoofed manifest cannot
//! inject hashes for attacker-controlled binaries.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, bail};
use clap::Parser;
use indexmap::IndexMap;
use lazy_regex::regex_replace_all;
use serde::{Deserialize, Serialize};

use crate::{cmd, http, pins_file, sri};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Spec {
    /// Release bucket base URL.
    pub base: String,
    /// Pinned ASCII-armored release signing key (a store path).
    pub signing_key: PathBuf,
    /// Ordered system-to-slug pairs; manifest.json platform order follows
    /// this list (a JSON list because Nix attrsets serialize sorted).
    pub platforms: Vec<Platform>,
    /// Repo-relative manifest.json to rewrite.
    pub manifest: PathBuf,
    /// Repo-relative directory of prompt snapshots (holds models.json).
    pub prompts: PathBuf,
}

#[derive(Deserialize)]
pub struct Platform {
    system: String,
    slug: String,
}

/// Run from the repo root: `nix run .#claude-code.updateScript -- [version]`.
/// Without a version argument it tracks Anthropic's `latest` pointer.
#[derive(Parser)]
#[command(name = "claude-code-update")]
struct Args {
    /// Exact release version; defaults to the upstream `latest` pointer.
    version: Option<String>,
    /// Only recapture prompt snapshots for the already-pinned package.
    #[arg(long, conflicts_with = "skip_prompts")]
    prompts_only: bool,
    /// Only move the signed binary manifest.
    #[arg(long)]
    skip_prompts: bool,
}

pub fn run(spec: &Spec, args: &[String]) -> Result<()> {
    let program = std::iter::once("claude-code-update");
    let args = Args::parse_from(program.chain(args.iter().map(String::as_str)));
    if args.prompts_only {
        return refresh_prompts(&spec.prompts);
    }

    let version = match args.version {
        Some(version) => version,
        None => http::get_text(&format!("{}/latest", spec.base))?
            .trim()
            .to_owned(),
    };

    // Download the exact bytes we verify, then parse the same bytes.
    let work = tempfile::tempdir().context("creating manifest workdir")?;
    let manifest_path = work.path().join("manifest.json");
    let sig_path = work.path().join("manifest.json.sig");
    let manifest_bytes = http::get_bytes(&format!("{}/{version}/manifest.json", spec.base))?;
    std::fs::write(&manifest_path, &manifest_bytes).context("writing manifest workdir copy")?;
    let sig_bytes = http::get_bytes(&format!("{}/{version}/manifest.json.sig", spec.base))?;
    std::fs::write(&sig_path, sig_bytes).context("writing signature workdir copy")?;

    verify_signature(&spec.signing_key, &sig_path, &manifest_path, &version)?;

    let upstream: UpstreamManifest = serde_json::from_slice(&manifest_bytes)
        .with_context(|| format!("parsing upstream manifest for {version}"))?;
    let platforms = spec
        .platforms
        .iter()
        .map(|platform| {
            let upstream_platform = upstream
                .platforms
                .get(&platform.slug)
                .with_context(|| format!("upstream manifest lists no platform {}", platform.slug))?;
            Ok((
                platform.system.clone(),
                PinnedPlatform {
                    slug: platform.slug.clone(),
                    hash: sri::from_hex(&upstream_platform.checksum)?,
                },
            ))
        })
        .collect::<Result<IndexMap<String, PinnedPlatform>>>()?;

    pins_file::write(
        &spec.manifest,
        &PinnedManifest {
            version: version.clone(),
            platforms,
        },
    )?;
    println!(
        "updated {} to {version}; signature verified",
        spec.manifest.display()
    );

    if !args.skip_prompts {
        refresh_prompts(&spec.prompts)?;
    }
    Ok(())
}

/// Fail closed: only the pinned key lives in this GNUPGHOME, so a zero exit
/// from `--verify` proves Anthropic signed these exact bytes.
fn verify_signature(signing_key: &Path, sig: &Path, manifest: &Path, version: &str) -> Result<()> {
    let gnupghome = tempfile::tempdir().context("creating GNUPGHOME")?;
    cmd::stdout(
        Command::new("gpg")
            .env("GNUPGHOME", gnupghome.path())
            .args(["--batch", "--quiet", "--import"])
            .arg(signing_key),
    )?;

    let output = Command::new("gpg")
        .env("GNUPGHOME", gnupghome.path())
        .args(["--batch", "--verify"])
        .args([sig, manifest])
        .output()
        .context("spawning gpg --verify")?;
    if !output.status.success() {
        bail!(
            "claude-code: manifest signature verification failed for {version}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn refresh_prompts(prompts_dir: &Path) -> Result<()> {
    let models_path = prompts_dir.join("models.json");
    let raw = std::fs::read_to_string(&models_path)
        .with_context(|| format!("reading {}", models_path.display()))?;
    let models: IndexMap<String, String> =
        serde_json::from_str(&raw).with_context(|| format!("parsing {}", models_path.display()))?;

    for (name, model) in &models {
        let raw = cmd::stdout(Command::new("nix").args([
            "run",
            ".#claude-code.extractStockSystemPrompt",
            "--",
            "--mode",
            "stock",
            "--model",
            model,
            "--json",
        ]))
        .with_context(|| format!("claude-code: failed to capture {name} system prompt"))?;
        let capture: Capture = serde_json::from_str(&raw)
            .with_context(|| format!("parsing {name} system prompt capture"))?;

        let out = prompts_dir.join(format!("{name}.txt"));
        std::fs::write(&out, format!("{}\n", scrub(&capture)))
            .with_context(|| format!("writing {}", out.display()))?;
        println!("updated {} from model {model}", out.display());
    }
    Ok(())
}

/// Join the prompt blocks, dropping the billing-header block and normalizing
/// the randomized extraction sandbox paths so reruns are byte-stable.
fn scrub(capture: &Capture) -> String {
    let joined = capture
        .system
        .iter()
        .filter(|block| !block.text.starts_with("x-anthropic-billing-header:"))
        .map(|block| block.text.as_str())
        .collect::<Vec<&str>>()
        .join("\n");
    let home = regex_replace_all!(
        "claude-extract-home[-_][A-Za-z0-9_-]+",
        &joined,
        "claude-extract-home"
    );
    regex_replace_all!(
        "claude-extract-cwd[-_][A-Za-z0-9_-]+",
        &home,
        "claude-extract-cwd"
    )
    .into_owned()
}

#[derive(Deserialize)]
struct UpstreamManifest {
    platforms: std::collections::HashMap<String, UpstreamPlatform>,
}

#[derive(Deserialize)]
struct UpstreamPlatform {
    checksum: String,
}

#[derive(Serialize)]
struct PinnedManifest {
    version: String,
    platforms: IndexMap<String, PinnedPlatform>,
}

#[derive(Serialize)]
struct PinnedPlatform {
    slug: String,
    hash: String,
}

/// The `--json` capture shape of `extractStockSystemPrompt`: the prompt as
/// ordered content blocks.
#[derive(Deserialize)]
struct Capture {
    system: Vec<Block>,
}

#[derive(Deserialize)]
struct Block {
    text: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrub_drops_billing_header_and_normalizes_paths() {
        let capture = Capture {
            system: vec![
                Block {
                    text: "x-anthropic-billing-header: abc".to_owned(),
                },
                Block {
                    text: "You are Claude Code in /tmp/claude-extract-home-Xy_9/w".to_owned(),
                },
                Block {
                    text: "cwd is claude-extract-cwd_A1-b".to_owned(),
                },
            ],
        };
        assert_eq!(
            scrub(&capture),
            "You are Claude Code in /tmp/claude-extract-home/w\ncwd is claude-extract-cwd"
        );
    }
}
