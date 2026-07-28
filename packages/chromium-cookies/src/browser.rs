//! Where a Chromium app keeps its cookies and what its Keychain secret is called.
//!
//! Every Chromium app follows the same shape, so resolution is a search, not a
//! hardcoded table. A name is either a display name we know an alias for, or the
//! literal `Application Support` subdirectory of the app.

use anyhow::{Result, anyhow, bail};
use std::path::{Path, PathBuf};

/// A resolved app: the cookie DB to open and the Keychain secret to unlock it.
#[derive(Debug, Clone)]
pub struct Target {
    pub name: String,
    pub cookie_db: PathBuf,
    pub keychain_service: String,
}

/// Display-name aliases whose `Application Support` dir differs from the name.
fn alias(name: &str) -> &'static str {
    match name.to_ascii_lowercase().as_str() {
        "chrome" | "google chrome" => "Google/Chrome",
        "chrome beta" => "Google/Chrome Beta",
        "chromium" => "Chromium",
        "edge" => "Microsoft Edge",
        "brave" => "BraveSoftware/Brave-Browser",
        "arc" => "Arc",
        "dia" => "Dia",
        _ => "",
    }
}

/// The Keychain secret is `<DisplayName> Safe Storage`; recover the display name.
fn keychain_service_for(name: &str) -> String {
    let display = match name.to_ascii_lowercase().as_str() {
        "chrome" | "google chrome" => "Chrome",
        "chromium" => "Chromium",
        "edge" => "Microsoft Edge",
        "brave" => "Brave",
        "arc" => "Arc",
        "dia" => "Dia",
        _ => name,
    };
    format!("{display} Safe Storage")
}

fn app_support() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME is unset"))?;
    Ok(Path::new(&home).join("Library/Application Support"))
}

/// Candidate cookie-DB locations under an app dir, most specific first. Chrome
/// uses a flat profile; Arc and Dia nest it under `User Data`.
fn cookie_db_candidates(dir: &Path) -> Vec<PathBuf> {
    [
        "User Data/Default/Network/Cookies",
        "User Data/Default/Cookies",
        "Default/Network/Cookies",
        "Default/Cookies",
        "Network/Cookies",
        "Cookies",
    ]
    .iter()
    .map(|leaf| dir.join(leaf))
    .collect()
}

/// Resolve a name to a concrete, existing cookie store plus its Keychain secret.
pub fn resolve(name: &str) -> Result<Target> {
    let base = app_support()?;
    let subdir = match alias(name) {
        "" => name,
        mapped => mapped,
    };
    let dir = base.join(subdir);
    let cookie_db = cookie_db_candidates(&dir)
        .into_iter()
        .find(|p| p.exists())
        .ok_or_else(|| {
            anyhow!(
                "no cookie DB under {} (looked for a Chromium profile, e.g. User Data/Default/Cookies)",
                dir.display()
            )
        })?;

    Ok(Target {
        name: name.to_owned(),
        cookie_db,
        keychain_service: keychain_service_for(name),
    })
}

/// Every Chromium app on this machine that has a cookie DB, by subdir name.
pub fn discover() -> Result<Vec<Target>> {
    let base = app_support()?;
    let entries = std::fs::read_dir(&base).map_err(|e| anyhow!("reading {}: {e}", base.display()))?;

    let mut out: Vec<Target> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let db = cookie_db_candidates(&entry.path())
                .into_iter()
                .find(|p| p.exists())?;
            let name = entry.file_name().to_string_lossy().into_owned();
            Some(Target {
                keychain_service: keychain_service_for(&name),
                name,
                cookie_db: db,
            })
        })
        .collect();

    if out.is_empty() {
        bail!("no Chromium cookie stores found under {}", base.display());
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}
