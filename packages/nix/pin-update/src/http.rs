//! Tiny blocking HTTP surface over reqwest (rustls, webpki roots), enough
//! for the `PyPI` JSON API and Anthropic's release bucket.

use anyhow::{Context as _, Result};
use serde::de::DeserializeOwned;

fn get(url: &str) -> Result<reqwest::blocking::Response> {
    reqwest::blocking::get(url)
        .and_then(reqwest::blocking::Response::error_for_status)
        .with_context(|| format!("GET {url}"))
}

pub fn get_json<T: DeserializeOwned>(url: &str) -> Result<T> {
    get(url)?
        .json()
        .with_context(|| format!("parsing GET {url} as JSON"))
}

pub fn get_text(url: &str) -> Result<String> {
    get(url)?
        .text()
        .with_context(|| format!("reading GET {url}"))
}

pub fn get_bytes(url: &str) -> Result<Vec<u8>> {
    Ok(get(url)?
        .bytes()
        .with_context(|| format!("reading GET {url}"))?
        .to_vec())
}
