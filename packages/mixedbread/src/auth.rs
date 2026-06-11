//! Credential resolution.
//!
//! Prefers the `MXBAI_API_KEY` environment variable. Otherwise it reuses the
//! credential written by `mgrep login` at `~/.mgrep/token.json`: that file
//! holds an OAuth access token, which is exchanged for a short-lived API JWT at
//! the platform auth endpoint. The JWT is what authenticates against the API.

use std::path::PathBuf;

use serde::Deserialize;
use snafu::{OptionExt as _, ResultExt as _};

use crate::{
    API_KEY_ENV, BuildClientSnafu, CredentialParseSnafu, EmptyJwtSnafu, HttpSnafu,
    NoCredentialSnafu, ReadCredentialSnafu, Result, TokenExchangeSnafu,
};

/// Platform base URL where the stored OAuth token is exchanged for an API JWT.
/// Matches the endpoint the `mgrep` CLI uses.
pub const PLATFORM_URL: &str = "https://www.platform.mixedbread.com";

#[derive(Deserialize)]
struct StoredToken {
    access_token: String,
}

#[derive(Deserialize)]
struct JwtResponse {
    #[serde(default)]
    token: Option<String>,
}

/// Resolve an API bearer token, preferring `MXBAI_API_KEY`, then the
/// `mgrep login` credential exchanged for a JWT at `platform_url`.
///
/// # Errors
/// Returns an error if no credential is found, the token file cannot be read or
/// parsed, or the exchange request fails or returns no token.
pub async fn resolve_token(platform_url: &str) -> Result<String> {
    if let Ok(key) = std::env::var(API_KEY_ENV)
        && !key.is_empty()
    {
        return Ok(key);
    }

    let path = mgrep_token_path().context(NoCredentialSnafu)?;
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return NoCredentialSnafu.fail();
        }
        Err(err) => return Err(err).context(ReadCredentialSnafu { path }),
    };
    let stored: StoredToken =
        serde_json::from_slice(&bytes).context(CredentialParseSnafu { path })?;

    exchange_for_jwt(platform_url, &stored.access_token).await
}

/// Path to the `mgrep login` token file, `~/.mgrep/token.json`.
#[must_use]
pub fn mgrep_token_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".mgrep").join("token.json"))
}

async fn exchange_for_jwt(platform_url: &str, access_token: &str) -> Result<String> {
    let client = crate::bounded_http_builder()
        .build()
        .context(BuildClientSnafu)?;
    let resp = client
        .get(format!("{platform_url}/api/auth/token"))
        .bearer_auth(access_token)
        .send()
        .await
        .context(HttpSnafu)?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return TokenExchangeSnafu { status, body }.fail();
    }

    let jwt: JwtResponse = resp.json().await.context(HttpSnafu)?;
    jwt.token.context(EmptyJwtSnafu)
}

#[cfg(test)]
mod tests {
    use super::mgrep_token_path;

    #[test]
    fn token_path_points_at_dot_mgrep() {
        if let Some(path) = mgrep_token_path() {
            assert!(path.ends_with(".mgrep/token.json"));
        }
    }
}
