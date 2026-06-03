//! Embed the dashboard's single-page UI without committing the build artifact.
//!
//! The Svelte/Vite app under `site/` builds to one self-contained `index.html`
//! (viteSingleFile). Nix builds it (`ix.buildSvelteSite`) and points the
//! workspace at the file through `IX_DASHBOARD_SITE_HTML` (set workspace-wide;
//! only this crate reads it, the same shape as `IX_VT_GHOSTTY_LIB_DIR` for
//! `libghostty-vt`). We copy that file into `OUT_DIR` so `server.rs` can
//! `include_str!` it at compile time.
//!
//! Compile-time embedding is deliberate: `dashboard-core` is linked into
//! non-wrappable artifacts (the `tui-py` `PyO3` `.so` the MCP loads), which can't
//! carry a runtime `--site-dir` like a standalone binary would. Embedding keeps
//! every consumer self-contained with no runtime asset dependency, while nix,
//! not git, owns the generated page.
//!
//! When the env var is unset (a bare `cargo build` outside nix), a small stub is
//! embedded so the crate still compiles; real builds always go through nix.

use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;

/// Names the prebuilt `index.html`. The owning Nix derivation sets this.
const SITE_HTML_ENV: &str = "IX_DASHBOARD_SITE_HTML";

const STUB_HTML: &str = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<title>dashboard</title></head><body style=\"font:14px system-ui;padding:2rem\">\
<p>The dashboard UI was not built. Build through nix (e.g. <code>nix build .#dashboard</code>), \
which sets IX_DASHBOARD_SITE_HTML to the Vite output.</p></body></html>";

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-env-changed={SITE_HTML_ENV}");

    let out = PathBuf::from(env::var_os("OUT_DIR").ok_or("OUT_DIR not set")?).join("dashboard.html");

    match env::var_os(SITE_HTML_ENV) {
        Some(path) if !path.is_empty() => {
            let path = PathBuf::from(path);
            println!("cargo:rerun-if-changed={}", path.display());
            let html = fs::read(&path)
                .map_err(|source| format!("read {SITE_HTML_ENV}={}: {source}", path.display()))?;
            fs::write(&out, html)?;
        }
        _ => fs::write(&out, STUB_HTML)?,
    }

    Ok(())
}
