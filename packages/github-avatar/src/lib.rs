//! Resolve a git commit author to a GitHub user and fetch their avatar.
//!
//! Resolution is layered so the cheap, offline paths run first:
//!
//! 1. [`parse_noreply`] reads the login straight out of a
//!    `…@users.noreply.github.com` commit email, with no network.
//! 2. [`Client::resolve_commit`] asks GitHub who authored a specific commit in a
//!    repo, which resolves any email linked to an account.
//!
//! Once a login is known, [`Client::avatar_png`] downloads the avatar as `PNG`
//! from `<https://github.com/LOGIN.png>`, which always returns `PNG` regardless
//! of the format the user originally uploaded.

use std::io::Cursor;
use std::time::Duration;

use reqwest::StatusCode;
use serde::Deserialize;
use snafu::{ResultExt, Snafu, ensure};

/// GitHub REST API version pin (sent as `X-GitHub-Api-Version`).
const API_VERSION: &str = "2022-11-28";
/// `User-Agent` sent on every request; GitHub rejects requests without one.
const USER_AGENT: &str = concat!("git-log-pretty/", env!("CARGO_PKG_VERSION"));

/// A resolved GitHub account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub login: String,
}

/// Failure talking to GitHub.
#[derive(Debug, Snafu)]
pub enum Error {
    /// A request failed to send, returned a non-success status, or its body
    /// could not be decoded.
    #[snafu(display("github request to {url} failed"))]
    Request {
        url: String,
        source: reqwest::Error,
    },
    /// A login failed validation, so no request was made.
    #[snafu(display("{login:?} is not a valid github login"))]
    InvalidLogin { login: String },
    /// The downloaded avatar could not be decoded as an image.
    #[snafu(display("failed to decode {login}'s avatar image"))]
    Decode {
        login: String,
        source: image::ImageError,
    },
    /// The avatar could not be re-encoded as PNG.
    #[snafu(display("failed to encode {login}'s avatar as PNG"))]
    Encode {
        login: String,
        source: image::ImageError,
    },
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Parse a GitHub `noreply` commit email into a [`User`].
///
/// Handles both forms GitHub issues:
/// `49699333+octocat@users.noreply.github.com` and the older
/// `octocat@users.noreply.github.com`. Returns `None` for any other email, or
/// when the embedded login is not [valid](is_valid_login) (the local part is
/// attacker-controlled, so this is what keeps stray characters out of the avatar
/// URL later).
#[must_use]
pub fn parse_noreply(email: &str) -> Option<User> {
    let local = email
        .trim()
        .to_ascii_lowercase()
        .strip_suffix("@users.noreply.github.com")?
        .to_string();
    // Newer emails are "<id>+<login>"; the id half is not a login on its own.
    let login = match local.split_once('+') {
        Some((_, login)) => login,
        None => local.as_str(),
    };
    is_valid_login(login).then(|| User {
        login: login.to_string(),
    })
}

/// Whether `login` is a syntactically valid GitHub username: 1 to 39 characters
/// of ASCII alphanumerics and hyphens, not starting or ending with a hyphen.
///
/// A valid login needs no URL or path encoding, so validating here lets callers
/// interpolate it safely. Used as a guard before any network use.
#[must_use]
pub fn is_valid_login(login: &str) -> bool {
    !login.is_empty()
        && login.len() <= 39
        && login.bytes().next() != Some(b'-')
        && login.bytes().next_back() != Some(b'-')
        && login.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// An `owner/repo` pair identifying a GitHub repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSlug {
    /// The repository owner (user or org).
    pub owner: String,
    /// The repository name.
    pub repo: String,
}

/// Parse the [`RepoSlug`] from a GitHub remote URL (https or ssh forms).
///
/// Returns `None` for non-GitHub remotes, so callers can skip the commit-author
/// lookup entirely off GitHub.
#[must_use]
pub fn parse_remote(url: &str) -> Option<RepoSlug> {
    let url = url.trim();
    let url = url.strip_suffix(".git").unwrap_or(url);
    let rest = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))
        .or_else(|| url.strip_prefix("git@github.com:"))?;
    let (owner, repo) = rest.split_once('/')?;
    (!owner.is_empty() && !repo.is_empty())
        .then(|| RepoSlug { owner: owner.to_string(), repo: repo.to_string() })
}

/// Authenticated response for `GET /repos/{owner}/{repo}/commits/{sha}`; only
/// the linked author account is of interest.
#[derive(Deserialize)]
struct CommitResponse {
    author: Option<Account>,
}

#[derive(Deserialize)]
struct Account {
    login: String,
}

/// A GitHub HTTP client.
///
/// Holds one connection pool and an optional token used for the authenticated
/// API lookups (raising rate limits and reaching private repos). The avatar
/// download itself needs no token.
pub struct Client {
    http: reqwest::Client,
    token: Option<String>,
}

impl Client {
    /// Build a client. Pass a token (e.g. from `GITHUB_TOKEN` or `gh auth
    /// token`) to enable the API lookups; pass `None` to use only the public
    /// avatar endpoint.
    #[must_use]
    pub fn new(token: Option<String>) -> Self {
        // A per-request timeout keeps a stalled api.github.com / github.com
        // response from hanging the synchronous caller indefinitely; on a build
        // failure (e.g. TLS init) fall back to the untimed default client.
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { http, token }
    }

    /// Resolve the GitHub login that authored `sha` in `owner/repo`.
    ///
    /// Returns `Ok(None)` when GitHub has no account linked to the commit's
    /// author email, or when the commit is not found.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Request`] if the request fails to send, returns an
    /// unexpected non-success status, or its body cannot be decoded.
    pub async fn resolve_commit(&self, owner: &str, repo: &str, sha: &str) -> Result<Option<User>> {
        let url = format!("https://api.github.com/repos/{owner}/{repo}/commits/{sha}");
        let response = self.api_get(&url).await?;
        // A missing commit or unprocessable ref is "no answer", not an error.
        if matches!(response.status(), StatusCode::NOT_FOUND | StatusCode::UNPROCESSABLE_ENTITY) {
            return Ok(None);
        }
        let response = response.error_for_status().context(RequestSnafu { url: url.clone() })?;
        let parsed: CommitResponse = response.json().await.context(RequestSnafu { url })?;
        Ok(parsed.author.map(|account| User { login: account.login }))
    }

    /// Download `login`'s avatar and return it as `PNG` bytes, a `size_px`
    /// square.
    ///
    /// GitHub's `.png` endpoint sometimes serves the original upload (`JPEG`,
    /// `WebP`, `GIF`, …) rather than `PNG`. kitty's `f=100` format needs `PNG`,
    /// so whatever comes back is decoded, resized to a square, and re-encoded as
    /// `PNG`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidLogin`] if `login` is not a valid username,
    /// [`Error::Request`] if the download fails or returns a non-success status,
    /// or [`Error::Decode`] / [`Error::Encode`] if the image cannot be
    /// transcoded.
    pub async fn avatar_png(&self, login: &str, size_px: u32) -> Result<Vec<u8>> {
        ensure!(is_valid_login(login), InvalidLoginSnafu { login });
        let url = format!("https://github.com/{login}.png?size={size_px}");
        let bytes = self
            .http
            .get(&url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .send()
            .await
            .context(RequestSnafu { url: url.clone() })?
            .error_for_status()
            .context(RequestSnafu { url: url.clone() })?
            .bytes()
            .await
            .context(RequestSnafu { url })?;

        let img = image::load_from_memory(&bytes).context(DecodeSnafu { login })?;
        let img = img.resize_exact(size_px, size_px, image::imageops::FilterType::Triangle);
        let mut png = Vec::new();
        img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .context(EncodeSnafu { login })?;
        Ok(png)
    }

    /// Issue an authenticated GitHub API GET with the standard headers.
    async fn api_get(&self, url: &str) -> Result<reqwest::Response> {
        let mut request = self
            .http
            .get(url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", API_VERSION);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        request.send().await.context(RequestSnafu { url: url.to_string() })
    }
}

#[cfg(test)]
mod tests {
    use super::{RepoSlug, User, is_valid_login, parse_noreply, parse_remote};

    #[test]
    fn noreply_with_and_without_id() {
        let want = Some(User { login: "octocat".to_string() });
        assert_eq!(parse_noreply("49699333+octocat@users.noreply.github.com"), want);
        assert_eq!(parse_noreply("octocat@users.noreply.github.com"), want);
        assert_eq!(parse_noreply("Octocat@Users.Noreply.GitHub.com"), want);
    }

    #[test]
    fn non_noreply_and_unsafe_logins_are_rejected() {
        assert_eq!(parse_noreply("drew@x.ai"), None);
        assert_eq!(parse_noreply("nope"), None);
        // A crafted local part must not yield a URL-injecting login.
        assert_eq!(parse_noreply("a/b@users.noreply.github.com"), None);
        assert_eq!(parse_noreply("a?b@users.noreply.github.com"), None);
        assert_eq!(parse_noreply("dependabot[bot]@users.noreply.github.com"), None);
    }

    #[test]
    fn valid_login_charset_and_edges() {
        assert!(is_valid_login("octocat"));
        assert!(is_valid_login("andrew-gazelka"));
        assert!(is_valid_login("a"));
        assert!(!is_valid_login(""));
        assert!(!is_valid_login("-lead"));
        assert!(!is_valid_login("trail-"));
        assert!(!is_valid_login("has space"));
        assert!(!is_valid_login("has/slash"));
        assert!(!is_valid_login(&"x".repeat(40)));
    }

    #[test]
    fn remote_https_and_ssh() {
        let want = Some(RepoSlug { owner: "indexable-inc".to_string(), repo: "index".to_string() });
        assert_eq!(parse_remote("https://github.com/indexable-inc/index.git"), want);
        assert_eq!(parse_remote("https://github.com/indexable-inc/index"), want);
        assert_eq!(parse_remote("git@github.com:indexable-inc/index.git"), want);
        assert_eq!(parse_remote("ssh://git@github.com/indexable-inc/index.git"), want);
        assert_eq!(parse_remote("https://gitlab.com/foo/bar.git"), None);
    }
}
