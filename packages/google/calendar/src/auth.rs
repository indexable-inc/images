//! OAuth for the Calendar client: the installed-app consent flow and the
//! refresh-token exchange.
//!
//! The shape follows Google's installed-app guidance
//! (<https://developers.google.com/identity/protocols/oauth2/native-app>):
//! a team OAuth client (id + secret from the environment, sourced from
//! rbw/op), a per-person consent in a browser that redirects to a loopback
//! listener, PKCE (RFC 7636) binding the code to this process, and an offline
//! refresh token stored in a user-only file. Later calls mint short-lived
//! access tokens from that refresh token; no third-party broker sits in the
//! path (#599).
//!
//! On a headless host (SSH into a VM) the loopback redirect lands on the
//! browser's machine instead and fails to connect there; the full redirect
//! URL in the browser's address bar still carries the code, so the flow also
//! accepts that URL pasted back (see
//! [`PendingConsent::code_from_redirect_url`]).

use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use snafu::{OptionExt as _, ResultExt as _, ensure};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use url::Url;
use uuid::Uuid;

use crate::error::{
    ConsentDeniedSnafu, Error, HttpSnafu, ListenSnafu, MissingClientIdSnafu,
    MissingClientSecretSnafu, MissingCodeSnafu, MissingRefreshTokenSnafu, NoConfigDirSnafu,
    NoTokenSnafu, ParseTokenSnafu, ReadTokenSnafu, RedirectParseSnafu, Result, StateMismatchSnafu,
    TokenExchangeSnafu, TokenRevokedSnafu, WriteTokenSnafu,
};

/// Environment variable holding the OAuth client id.
pub const CLIENT_ID_ENV: &str = "GOOGLE_OAUTH_CLIENT_ID";

/// Environment variable holding the OAuth client secret.
pub const CLIENT_SECRET_ENV: &str = "GOOGLE_OAUTH_CLIENT_SECRET";

/// The events read/write scope: enough for list/get/create/cancel, without
/// access to calendar settings or the user's calendar list.
pub const EVENTS_SCOPE: &str = "https://www.googleapis.com/auth/calendar.events";

/// Gmail read/modify scope: read messages, labels, and threads, and change
/// labels/read-state (archive, trash). Does not cover sending; pair it with
/// [`GMAIL_SEND_SCOPE`] for that.
pub const GMAIL_MODIFY_SCOPE: &str = "https://www.googleapis.com/auth/gmail.modify";

/// Gmail send scope: send mail as the authenticated user.
pub const GMAIL_SEND_SCOPE: &str = "https://www.googleapis.com/auth/gmail.send";

/// Google's OAuth consent endpoint.
const AUTH_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";

/// Google's OAuth token endpoint (code exchange and refresh). Overridable per
/// instance with `with_token_endpoint` (tests).
const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";

/// Cap on one loopback HTTP request head; a redirect GET is well under this.
const MAX_REDIRECT_REQUEST: usize = 8 * 1024;

/// The team OAuth client identity.
///
/// No `Debug`: the secret should not ride along into logs or panic messages.
#[derive(Clone)]
pub struct ClientSecrets {
    /// OAuth client id.
    pub client_id: String,
    /// OAuth client secret. For an installed app this is not a true secret
    /// (the binary's user can always extract it), but it stays out of the
    /// repo and comes from the team secret store.
    pub client_secret: String,
}

impl ClientSecrets {
    /// Read the client identity from [`CLIENT_ID_ENV`] and
    /// [`CLIENT_SECRET_ENV`].
    ///
    /// # Errors
    /// Returns an error naming the missing variable if either is unset or
    /// empty.
    pub fn from_env() -> Result<Self> {
        let client_id = non_empty_env(CLIENT_ID_ENV).context(MissingClientIdSnafu)?;
        let client_secret = non_empty_env(CLIENT_SECRET_ENV).context(MissingClientSecretSnafu)?;
        Ok(Self {
            client_id,
            client_secret,
        })
    }
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// The persisted grant: the offline refresh token and the scopes it covers.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredToken {
    /// The offline refresh token.
    pub refresh_token: String,
    /// Scopes granted with it.
    pub scopes: Vec<String>,
}

/// Redacts the refresh token: `Debug` output reaches assertion messages and
/// error context, and the token is the long-lived credential.
impl std::fmt::Debug for StoredToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredToken")
            .field("refresh_token", &"<redacted>")
            .field("scopes", &self.scopes)
            .finish()
    }
}

/// Owner of the token file.
#[derive(Debug, Clone)]
pub struct TokenStore {
    path: PathBuf,
}

impl TokenStore {
    /// The store at the default per-user location,
    /// `<config dir>/gcal/token.json` (`~/.config/gcal/token.json` on Linux).
    ///
    /// # Errors
    /// Returns an error if the platform exposes no config directory.
    pub fn new() -> Result<Self> {
        let config = dirs::config_dir().context(NoConfigDirSnafu)?;
        Ok(Self::at(config.join("gcal").join("token.json")))
    }

    /// The store at an explicit path (tests, alternate deployments).
    #[must_use]
    pub const fn at(path: PathBuf) -> Self {
        Self { path }
    }

    /// Where this store reads and writes.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load the stored grant.
    ///
    /// # Errors
    /// Returns [`crate::Error::NoToken`] (run `gcal auth`) if the file does
    /// not exist, and read/parse errors otherwise.
    pub fn load(&self) -> Result<StoredToken> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return NoTokenSnafu {
                    path: self.path.clone(),
                }
                .fail();
            }
            Err(err) => {
                return Err(err).context(ReadTokenSnafu {
                    path: self.path.clone(),
                });
            }
        };
        serde_json::from_slice(&bytes).context(ParseTokenSnafu {
            path: self.path.clone(),
        })
    }

    /// Persist a grant, creating parent directories and keeping the file
    /// user-only (mode 0600): it holds a long-lived credential.
    ///
    /// # Errors
    /// Returns an error if the directory or file cannot be written.
    ///
    /// # Panics
    /// Never in practice: serializing [`StoredToken`] (plain strings) cannot
    /// fail.
    pub fn save(&self, token: &StoredToken) -> Result<()> {
        use std::io::Write as _;
        use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).context(WriteTokenSnafu {
                path: self.path.clone(),
            })?;
        }
        let body = serde_json::to_vec_pretty(token)
            .expect("StoredToken serialization cannot fail: plain strings");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&self.path)
            .context(WriteTokenSnafu {
                path: self.path.clone(),
            })?;
        // `mode` above only applies when the file is created; an existing
        // file (for example from a looser earlier writer) keeps its old mode,
        // so tighten the open handle unconditionally before the token lands.
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .context(WriteTokenSnafu {
                path: self.path.clone(),
            })?;
        file.write_all(&body).context(WriteTokenSnafu {
            path: self.path.clone(),
        })
    }
}

/// What the token endpoint returns for both the code exchange and a refresh.
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    /// The granted scopes as the endpoint reports them. Read on the initial
    /// code exchange to record what was granted; ignored on refresh, where the
    /// authoritative scopes come from the stored grant (a refresh response often
    /// omits `scope`).
    #[serde(default)]
    scope: Option<String>,
    /// Access-token lifetime in seconds, when the endpoint reports one.
    #[serde(default)]
    expires_in: Option<u64>,
}

/// The token endpoint's error body (RFC 6749 §5.2).
#[derive(Deserialize)]
struct TokenErrorBody {
    error: String,
}

/// A non-success answer from the token endpoint, decoded once. Callers map it
/// onto their own failure: a dead grant reads differently mid-`gcal auth`
/// (the code expired) than on refresh (the consent itself is gone).
struct TokenDenied {
    status: u16,
    body: String,
    /// The RFC 6749 §5.2 error code, when the body carried one.
    error_code: Option<String>,
}

impl TokenDenied {
    /// Policy for the refresh path: `invalid_grant` means the refresh token
    /// itself is dead (revoked, expired, or consent withdrawn), so the fix is
    /// a new consent, not a retry.
    fn refresh_error(self) -> Error {
        if self.error_code.as_deref() == Some("invalid_grant") {
            TokenRevokedSnafu.build()
        } else {
            self.exchange_error()
        }
    }

    /// Policy for the code-exchange path: surface the endpoint's answer.
    fn exchange_error(self) -> Error {
        TokenExchangeSnafu {
            status: self.status,
            body: self.body,
        }
        .build()
    }
}

/// The outcome of one token-endpoint call.
enum TokenOutcome {
    Granted(TokenResponse),
    Denied(TokenDenied),
}

/// The token-endpoint half of OAuth: one owner for the client-authenticated
/// form POST and the grant/denial decoding that the code exchange and the
/// refresh share.
struct TokenClient {
    http: reqwest::Client,
    secrets: ClientSecrets,
    endpoint: String,
}

impl TokenClient {
    fn new(secrets: ClientSecrets) -> Result<Self> {
        Ok(Self {
            http: crate::http_client()?,
            secrets,
            endpoint: TOKEN_ENDPOINT.to_owned(),
        })
    }

    /// POST `grant_params` plus the client identity, and decode the answer.
    async fn post(&self, grant_params: &[(&str, &str)]) -> Result<TokenOutcome> {
        let mut form = vec![
            ("client_id", self.secrets.client_id.as_str()),
            ("client_secret", self.secrets.client_secret.as_str()),
        ];
        form.extend_from_slice(grant_params);

        let response = self
            .http
            .post(&self.endpoint)
            .form(&form)
            .send()
            .await
            .context(HttpSnafu)?;

        let status = response.status();
        if status.is_success() {
            let token = response.json().await.context(HttpSnafu)?;
            return Ok(TokenOutcome::Granted(token));
        }
        let body = response.text().await.unwrap_or_default();
        let error_code = serde_json::from_str::<TokenErrorBody>(&body)
            .ok()
            .map(|denied| denied.error);
        Ok(TokenOutcome::Denied(TokenDenied {
            status: status.as_u16(),
            body,
            error_code,
        }))
    }
}

/// A freshly minted access token plus the metadata a caller needs to cache it.
///
/// Returned by [`Authenticator::mint_access_token`]: the bearer token, its
/// reported lifetime, and the scopes the underlying grant covers (read from the
/// stored grant, since the refresh response often omits `scope`). The token is
/// the short-lived credential, so `Debug` redacts it.
#[derive(Clone)]
pub struct AccessToken {
    /// The bearer access token.
    pub token: String,
    /// Lifetime in seconds, when the token endpoint reported one.
    pub expires_in: Option<u64>,
    /// Scopes the grant covers (e.g. calendar.events, gmail.modify).
    pub scopes: Vec<String>,
}

impl std::fmt::Debug for AccessToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccessToken")
            .field("token", &"<redacted>")
            .field("expires_in", &self.expires_in)
            .field("scopes", &self.scopes)
            .finish()
    }
}

/// Mints access tokens from the stored refresh token.
///
/// One access token is fetched lazily per `Authenticator` and reused for its
/// lifetime; that matches the one-shot CLI and MCP-subprocess consumers, which
/// live far shorter than the token's hour. A long-lived daemon should hold one
/// `Authenticator` per operation rather than caching one across hours.
pub struct Authenticator {
    token: TokenClient,
    store: TokenStore,
    access: tokio::sync::OnceCell<String>,
}

impl Authenticator {
    /// An authenticator over the given identity and token store, against
    /// Google's token endpoint.
    ///
    /// # Errors
    /// Returns an error if the HTTP client cannot be built.
    pub fn new(secrets: ClientSecrets, store: TokenStore) -> Result<Self> {
        Ok(Self {
            token: TokenClient::new(secrets)?,
            store,
            access: tokio::sync::OnceCell::new(),
        })
    }

    /// Point at a different token endpoint (tests).
    #[must_use]
    pub fn with_token_endpoint(mut self, url: impl Into<String>) -> Self {
        self.token.endpoint = url.into();
        self
    }

    /// A current access token, minting one from the stored refresh token on
    /// first use.
    ///
    /// # Errors
    /// Returns [`crate::Error::NoToken`] when nothing is stored,
    /// [`crate::Error::TokenRevoked`] when the grant no longer works, and
    /// transport errors otherwise.
    pub async fn access_token(&self) -> Result<&str> {
        let token = self
            .access
            .get_or_try_init(|| async { self.refresh().await.map(|minted| minted.token) })
            .await?;
        Ok(token)
    }

    /// Mint a fresh access token (always a network refresh, never the cache),
    /// returning the token plus its lifetime and the grant's scopes. This is
    /// the path the `print-access-token` CLI uses to hand a current token to the
    /// bundled Python `google_auth` helper for Gmail/Calendar calls.
    ///
    /// # Errors
    /// Same as [`Self::access_token`]: [`crate::Error::NoToken`] when nothing is
    /// stored, [`crate::Error::TokenRevoked`] when the grant no longer works,
    /// and transport errors otherwise.
    pub async fn mint_access_token(&self) -> Result<AccessToken> {
        self.refresh().await
    }

    async fn refresh(&self) -> Result<AccessToken> {
        let stored = self.store.load()?;
        let outcome = self
            .token
            .post(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", stored.refresh_token.as_str()),
            ])
            .await?;

        let token = match outcome {
            TokenOutcome::Granted(token) => token,
            TokenOutcome::Denied(denied) => return Err(denied.refresh_error()),
        };
        let scopes = stored.scopes;
        if let Some(rotated) = token.refresh_token {
            // Google occasionally rotates the refresh token on refresh; the
            // old one stops working, so persist the replacement immediately.
            self.store.save(&StoredToken {
                refresh_token: rotated,
                scopes: scopes.clone(),
            })?;
        }
        Ok(AccessToken {
            token: token.access_token,
            expires_in: token.expires_in,
            scopes,
        })
    }
}

/// An authorization code captured from the consent redirect. One-shot and
/// deliberately opaque (redacted `Debug`, no accessor): it only travels into
/// [`PendingConsent::exchange`].
pub struct AuthCode(String);

impl std::fmt::Debug for AuthCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuthCode(redacted)")
    }
}

/// A consent attempt in flight: the URL for the user's browser, and the
/// loopback listener the redirect lands on. No `Debug`: it carries the client
/// secret and the PKCE verifier.
pub struct PendingConsent {
    /// The consent URL to open in a browser.
    pub auth_url: String,
    listener: TcpListener,
    redirect_uri: String,
    state: String,
    verifier: String,
    token: TokenClient,
}

/// Start a consent attempt for `scopes`: bind a loopback listener and build
/// the consent URL (offline access, forced consent so a refresh token is
/// issued, PKCE S256).
///
/// # Errors
/// Returns an error if the listener cannot bind or the HTTP client cannot be
/// built.
///
/// # Panics
/// Never in practice: the auth-endpoint constant is a valid URL.
pub async fn begin_consent(secrets: ClientSecrets, scopes: &[&str]) -> Result<PendingConsent> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .context(ListenSnafu)?;
    let port = listener.local_addr().context(ListenSnafu)?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}");

    let state = Uuid::new_v4().simple().to_string();
    // Two v4 UUIDs give 64 unreserved hex chars: inside RFC 7636's 43..=128
    // length window, with 244 bits of entropy.
    let verifier = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());

    let mut auth_url = Url::parse(AUTH_ENDPOINT).expect("AUTH_ENDPOINT is a valid URL");
    auth_url
        .query_pairs_mut()
        .append_pair("client_id", &secrets.client_id)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", &scopes.join(" "))
        .append_pair("access_type", "offline")
        .append_pair("prompt", "consent")
        .append_pair("state", &state)
        .append_pair("code_challenge", &challenge_for(&verifier))
        .append_pair("code_challenge_method", "S256");

    Ok(PendingConsent {
        auth_url: auth_url.into(),
        listener,
        redirect_uri,
        state,
        verifier,
        token: TokenClient::new(secrets)?,
    })
}

impl PendingConsent {
    /// Point at a different token endpoint (tests).
    #[must_use]
    pub fn with_token_endpoint(mut self, url: impl Into<String>) -> Self {
        self.token.endpoint = url.into();
        self
    }

    /// Wait for the browser redirect on the loopback listener and extract the
    /// authorization code. Non-redirect requests (favicon probes, stray
    /// connections) get a 404 and the wait continues.
    ///
    /// # Errors
    /// Returns an error if accepting fails, Google reports a consent error,
    /// or the redirect is malformed or from another attempt.
    pub async fn wait_loopback(&self) -> Result<AuthCode> {
        loop {
            let (mut stream, _) = self.listener.accept().await.context(ListenSnafu)?;
            let Some(target) = read_request_target(&mut stream).await? else {
                respond(&mut stream, "400 Bad Request", "Not an OAuth redirect.").await;
                continue;
            };
            let Ok(url) = Url::parse(&format!("http://127.0.0.1{target}")) else {
                respond(&mut stream, "400 Bad Request", "Not an OAuth redirect.").await;
                continue;
            };
            if !has_redirect_params(&url) {
                respond(&mut stream, "404 Not Found", "Not an OAuth redirect.").await;
                continue;
            }

            let code = self.extract_code(&url);
            let page = match &code {
                Ok(_) => "gcal is authorized. You can close this tab.".to_owned(),
                Err(err) => format!("Authorization failed: {err}"),
            };
            respond(&mut stream, "200 OK", &page).await;
            return code;
        }
    }

    /// Extract the authorization code from a pasted redirect URL (the
    /// headless path: the browser shows a connection error on
    /// `http://127.0.0.1:…` but its address bar holds the code).
    ///
    /// # Errors
    /// Returns an error if the input is not a URL, Google reports a consent
    /// error, or the code or state is missing or from another attempt.
    pub fn code_from_redirect_url(&self, pasted: &str) -> Result<AuthCode> {
        let url = Url::parse(pasted).ok().context(RedirectParseSnafu {
            input: pasted.to_owned(),
        })?;
        self.extract_code(&url)
    }

    /// Exchange the authorization code for tokens and return the grant to
    /// store. Consumes the attempt: a code is single-use.
    ///
    /// # Errors
    /// Returns an error if the exchange fails or the response carries no
    /// refresh token.
    pub async fn exchange(self, code: AuthCode) -> Result<StoredToken> {
        let outcome = self
            .token
            .post(&[
                ("code", code.0.as_str()),
                ("redirect_uri", self.redirect_uri.as_str()),
                ("grant_type", "authorization_code"),
                ("code_verifier", self.verifier.as_str()),
            ])
            .await?;

        let token = match outcome {
            TokenOutcome::Granted(token) => token,
            TokenOutcome::Denied(denied) => return Err(denied.exchange_error()),
        };
        let refresh_token = token.refresh_token.context(MissingRefreshTokenSnafu)?;
        let scopes = token
            .scope
            .map(|joined| joined.split_whitespace().map(ToOwned::to_owned).collect())
            .unwrap_or_default();
        Ok(StoredToken {
            refresh_token,
            scopes,
        })
    }

    fn extract_code(&self, url: &Url) -> Result<AuthCode> {
        let mut code = None;
        let mut state = None;
        let mut error = None;
        for (key, value) in url.query_pairs() {
            match key.as_ref() {
                "code" => code = Some(value.into_owned()),
                "state" => state = Some(value.into_owned()),
                "error" => error = Some(value.into_owned()),
                _ => {}
            }
        }

        if let Some(reason) = error {
            return ConsentDeniedSnafu { reason }.fail();
        }
        ensure!(
            state.as_deref() == Some(self.state.as_str()),
            StateMismatchSnafu
        );
        code.map(AuthCode).context(MissingCodeSnafu)
    }
}

/// Whether a parsed loopback request looks like the OAuth redirect (rather
/// than a favicon probe or stray connection).
fn has_redirect_params(url: &Url) -> bool {
    url.query_pairs()
        .any(|(key, _)| matches!(key.as_ref(), "code" | "error"))
}

/// Read one HTTP request head from the loopback connection and return its
/// request target (the `/?code=…` part), or `None` for garbage.
async fn read_request_target(stream: &mut TcpStream) -> Result<Option<String>> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0_u8; 1024];
    loop {
        let read = stream.read(&mut chunk).await.context(ListenSnafu)?;
        if read == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..read]);
        if buf.windows(4).any(|window| window == b"\r\n\r\n") || buf.len() >= MAX_REDIRECT_REQUEST {
            break;
        }
    }

    let head = String::from_utf8_lossy(&buf);
    let mut parts = head.lines().next().unwrap_or_default().split_whitespace();
    let (Some("GET"), Some(target)) = (parts.next(), parts.next()) else {
        return Ok(None);
    };
    Ok(Some(target.to_owned()))
}

/// Answer the browser, best-effort: the consent outcome is decided by the
/// parsed redirect, not by whether this write lands.
async fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let page = format!("<!doctype html><html><body><p>{body}</p></body></html>");
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {len}\r\nconnection: close\r\n\r\n{page}",
        len = page.len(),
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

/// The PKCE S256 challenge for a verifier (RFC 7636 §4.2).
fn challenge_for(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::{StoredToken, TokenStore, challenge_for};
    use crate::error::Error;

    #[test]
    fn pkce_challenge_matches_the_rfc_7636_vector() {
        // RFC 7636 appendix B.
        assert_eq!(
            challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
        );
    }

    #[test]
    fn token_store_round_trips_and_is_user_only() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = TempDir::new().expect("tempdir");
        let store = TokenStore::at(dir.path().join("nested").join("token.json"));
        let token = StoredToken {
            refresh_token: "1//refresh".to_owned(),
            scopes: vec![super::EVENTS_SCOPE.to_owned()],
        };

        store.save(&token).expect("save");
        let mode = std::fs::metadata(store.path())
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "token file must be user-only");
        assert_eq!(store.load().expect("load"), token);
    }

    #[test]
    fn save_tightens_an_existing_looser_file() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = TempDir::new().expect("tempdir");
        let store = TokenStore::at(dir.path().join("token.json"));
        // A pre-existing world-readable file: `OpenOptions::mode` alone would
        // keep 0644 on rewrite, leaking the rotated refresh token.
        std::fs::write(store.path(), b"{}").expect("seed file");
        std::fs::set_permissions(store.path(), std::fs::Permissions::from_mode(0o644))
            .expect("loosen");

        store
            .save(&StoredToken {
                refresh_token: "1//rotated".to_owned(),
                scopes: Vec::new(),
            })
            .expect("save over existing file");

        let mode = std::fs::metadata(store.path())
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "rewrite must tighten the mode");
    }

    #[test]
    fn missing_token_names_the_path_and_the_fix() {
        let dir = TempDir::new().expect("tempdir");
        let store = TokenStore::at(dir.path().join("token.json"));
        let err = store.load().expect_err("no token stored");
        assert!(matches!(err, Error::NoToken { .. }), "got {err:?}");
        assert!(
            err.to_string().contains("gcal auth"),
            "message must name the fix: {err}"
        );
    }
}
