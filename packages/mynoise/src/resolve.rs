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

/// Upper bound on band probing. Bands are 0-indexed and real generators top out
/// around 10 (`0a.ogg`..`9a.ogg`); this guards against an unbounded loop if the
/// server ever stops returning 404 for missing files.
const MAX_BANDS: u32 = 16;

/// True if `s` starts with a band reference: `<digits>a` or `<digits>b`. Pages
/// reference bands bare (`Data/RAIN/0a"`, the `.ogg` is appended in JS), while
/// images keep their extension (`fb.jpg`), so we require digits then `a`/`b`
/// then a non-alphanumeric boundary (end, quote, or `.ogg`) and reject e.g.
/// `0album` or `fb.jpg`.
fn is_band_file(s: &str) -> bool {
    let digits = s.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return false;
    }
    let Some(rest) = s[digits..].strip_prefix(['a', 'b']) else {
        return false;
    };
    rest.chars()
        .next()
        .is_none_or(|c| !c.is_ascii_alphanumeric())
}

/// Pull a generator's audio `CODE` out of its page HTML.
///
/// Generator pages embed their audio as `Data/<CODE>/<n>a.ogg` URLs, where the
/// code is upper-case alphanumerics plus underscores. We accept the first
/// `Data/<CODE>/` whose code is immediately followed by a band file: the first
/// bare `Data/` reference on a page is usually a share image
/// (`Data/RAIN/fb.jpg`) that only shares the audio folder by luck, so anchoring
/// on the band file is robust to pages whose first reference is a different
/// folder. Pure so it can be unit tested offline.
fn parse_data_code(html: &str) -> Option<String> {
    let marker = "Data/";
    html.match_indices(marker).find_map(|(i, _)| {
        let rest = &html[i + marker.len()..];
        // Code chars are ASCII, so byte length equals the char count taken.
        let code: String = rest
            .chars()
            .take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_')
            .collect();
        if code.is_empty() {
            return None;
        }
        let tail = rest[code.len()..].strip_prefix('/')?;
        is_band_file(tail).then_some(code)
    })
}

/// True if `url` returns a 2xx for a HEAD request.
async fn head_ok(client: &reqwest::Client, url: &str) -> bool {
    matches!(client.head(url).send().await, Ok(r) if r.status().is_success())
}

/// Resolve `name` to a generator data code.
///
/// Tries, in order: the name as a bare upper-cased code (verified by probing its
/// `0a.ogg`), then the name as a generator-page slug whose HTML is scraped for
/// the embedded `Data/<CODE>/`.
pub async fn resolve_code(client: &reqwest::Client, name: &str) -> Result<String> {
    let upper = name.to_ascii_uppercase();
    let looks_like_code = name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    // Band 0 is the lowest band and always present for a valid code.
    if looks_like_code && head_ok(client, &format!("{BASE}/Data/{upper}/0a.ogg")).await {
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
    std::fs::create_dir_all(&dir).with_context(|| format!("create cache dir {}", dir.display()))?;

    let mut bands = Vec::new();
    // Bands are 0-indexed: a generator serves `0a.ogg`..`Na.ogg`. Starting at 1
    // would drop the lowest band and shift every per-band gain by one.
    for n in 0..MAX_BANDS {
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
        // Write to a temp file then rename so a Ctrl-C mid-download (the normal
        // way to quit this tool) can't leave a truncated `.ogg` that the next
        // run reuses as if complete. Rename is atomic on the same filesystem.
        let tmp = file.with_extension("ogg.part");
        std::fs::write(&tmp, &bytes)
            .with_context(|| format!("write cache file {}", tmp.display()))?;
        std::fs::rename(&tmp, &file)
            .with_context(|| format!("finalize cache file {}", file.display()))?;
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
            // `take_while` stops at the `.` before `.php`, so the slug is clean.
            let slug: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                .collect();
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
    fn extracts_code_from_band_url() {
        let html = r#"<audio src="Data/RAIN/0a.ogg"></audio>"#;
        assert_eq!(parse_data_code(html).as_deref(), Some("RAIN"));
    }

    #[test]
    fn allows_digits_and_underscores() {
        let html = r"loadBand('Data/B_17/3a.ogg');";
        assert_eq!(parse_data_code(html).as_deref(), Some("B_17"));
    }

    #[test]
    fn skips_leading_share_image() {
        // Real pages reference the share image (`Data/<CODE>/fb.jpg`) before any
        // band file; the bare-prefix parse used to return from that match.
        let html = r#"<meta content="Data/RAIN/fb.jpg"><audio src="Data/RAIN/0a.ogg">"#;
        assert_eq!(parse_data_code(html).as_deref(), Some("RAIN"));
    }

    #[test]
    fn prefers_band_folder_over_unrelated_folder() {
        let html = r#"img "Data/SHARE/og.png" audio "Data/THUNDER/0a.ogg""#;
        assert_eq!(parse_data_code(html).as_deref(), Some("THUNDER"));
    }

    #[test]
    fn accepts_b_take_files() {
        assert_eq!(parse_data_code("Data/CALM/2b.ogg").as_deref(), Some("CALM"));
    }

    #[test]
    fn accepts_bare_band_token_without_extension() {
        // The real page shape: image first, then bare band tokens (no `.ogg`).
        let html = r#"["Data/RAIN/fb.jpg","Data/RAIN/0a","Data/RAIN/1a"]"#;
        assert_eq!(parse_data_code(html).as_deref(), Some("RAIN"));
    }

    #[test]
    fn rejects_non_band_word_after_code() {
        // `album` art shares the folder but is not a band.
        assert_eq!(parse_data_code(r#""Data/RAIN/album.jpg""#), None);
    }

    #[test]
    fn none_when_absent() {
        assert_eq!(parse_data_code("<html>no audio here</html>"), None);
    }

    #[test]
    fn none_when_no_band_file_follows() {
        // A `Data/<CODE>/` that is only ever an image must not resolve.
        assert_eq!(parse_data_code(r#"<img src="Data/RAIN/fb.jpg">"#), None);
    }
}
