//! Resolve a user-supplied generator name to its myNoise data `CODE`, then
//! download that generator's band loops.
//!
//! myNoise serves every generator's audio as static files under
//! `https://mynoise.net/Data/<CODE>/<n>a.ogg`, one OGG per frequency band (the
//! `a` files; `b` files are alternate takes the site randomizes for variety, we
//! always take `a`). There is no server-side generation, so "resolve" is just
//! finding the `CODE` and "download" is fetching `1a.ogg`, `2a.ogg`, ... until
//! one 404s.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

const BASE: &str = "https://mynoise.net";

/// Upper bound on band probing. Real generators top out around 10 bands; this
/// guards against an unbounded loop if the server ever stops returning 404 for
/// missing files.
const MAX_BANDS: u32 = 16;

/// Pull the first `Data/<CODE>/` path out of a generator page's HTML.
///
/// The generator pages embed their audio folder as `Data/CODE/...` URLs; the
/// code is upper-case alphanumerics plus underscores. Pure so it can be unit
/// tested offline.
fn parse_data_code(html: &str) -> Option<String> {
    let marker = "Data/";
    let start = html.find(marker)? + marker.len();
    let code: String = html[start..]
        .chars()
        .take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_')
        .collect();
    if code.is_empty() { None } else { Some(code) }
}

/// True if `url` returns a 2xx for a HEAD request.
async fn head_ok(client: &reqwest::Client, url: &str) -> bool {
    matches!(client.head(url).send().await, Ok(r) if r.status().is_success())
}

/// Resolve `name` to a generator data code.
///
/// Tries, in order: the name as a bare upper-cased code (verified by probing its
/// `1a.ogg`), then the name as a generator-page slug whose HTML is scraped for
/// the embedded `Data/<CODE>/`.
pub async fn resolve_code(client: &reqwest::Client, name: &str) -> Result<String> {
    let upper = name.to_ascii_uppercase();
    let looks_like_code = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_');
    if looks_like_code && head_ok(client, &format!("{BASE}/Data/{upper}/1a.ogg")).await {
        return Ok(upper);
    }

    let page = format!("{BASE}/NoiseMachines/{name}.php");
    let html = client
        .get(&page)
        .send()
        .await
        .with_context(|| format!("fetch generator page {page}"))?
        .error_for_status()
        .with_context(|| format!("generator page not found for {name:?}"))?
        .text()
        .await
        .context("read generator page body")?;
    parse_data_code(&html)
        .with_context(|| format!("could not find a Data/<CODE>/ reference on {page}"))
}

/// Download every band loop for `code` into `cache_dir`, returning the cached
/// file paths in band order. Files already present are reused.
pub async fn download_bands(
    client: &reqwest::Client,
    code: &str,
    cache_dir: &Path,
) -> Result<Vec<PathBuf>> {
    let dir = cache_dir.join(code);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("create cache dir {}", dir.display()))?;

    let mut bands = Vec::new();
    for n in 1..=MAX_BANDS {
        let file = dir.join(format!("{n}a.ogg"));
        if file.exists() {
            bands.push(file);
            continue;
        }
        let url = format!("{BASE}/Data/{code}/{n}a.ogg");
        let resp = client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("fetch band {url}"))?;
        // First missing band marks the end of the generator's band list.
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            break;
        }
        let bytes = resp
            .error_for_status()
            .with_context(|| format!("fetch band {url}"))?
            .bytes()
            .await
            .with_context(|| format!("read band body {url}"))?;
        std::fs::write(&file, &bytes)
            .with_context(|| format!("write cache file {}", file.display()))?;
        bands.push(file);
    }

    if bands.is_empty() {
        bail!("no band files found for code {code:?}");
    }
    Ok(bands)
}

/// List generator page slugs from the noise-machine index, sorted and de-duped.
pub async fn list_slugs(client: &reqwest::Client) -> Result<Vec<String>> {
    let url = format!("{BASE}/noiseMachines.php");
    let html = client
        .get(&url)
        .send()
        .await
        .context("fetch noise-machine index")?
        .error_for_status()
        .context("noise-machine index returned an error")?
        .text()
        .await
        .context("read noise-machine index body")?;

    let marker = "NoiseMachines/";
    let mut slugs: Vec<String> = html
        .match_indices(marker)
        .filter_map(|(i, _)| {
            let rest = &html[i + marker.len()..];
            let slug: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                .collect();
            // Drop the trailing `.php` if the slug captured it via a stray char.
            let slug = slug.strip_suffix(".php").unwrap_or(&slug).to_string();
            (!slug.is_empty()).then_some(slug)
        })
        .collect();
    slugs.sort_unstable();
    slugs.dedup();
    Ok(slugs)
}

#[cfg(test)]
mod tests {
    use super::parse_data_code;

    #[test]
    fn extracts_code_from_data_url() {
        let html = r#"<audio src="Data/RAIN/1a.ogg"></audio>"#;
        assert_eq!(parse_data_code(html).as_deref(), Some("RAIN"));
    }

    #[test]
    fn allows_digits_and_underscores() {
        let html = r"loadBand('Data/B_17/3a.ogg');";
        assert_eq!(parse_data_code(html).as_deref(), Some("B_17"));
    }

    #[test]
    fn none_when_absent() {
        assert_eq!(parse_data_code("<html>no audio here</html>"), None);
    }
}
