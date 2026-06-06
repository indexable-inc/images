//! Wire-level tests for the OAuth flow against a local mock of Google's
//! token endpoint. These defend the invariants a refactor could silently
//! break: PKCE, forced consent, state validation, refresh-token rotation,
//! the access-token cache, the revoked-grant mapping, and the legacy
//! `~/.config/gcal/token.json` migration shim.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::{Form, Json, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use google_auth::{
    Authenticator, ClientSecrets, Error, StoredToken, TokenStore, begin_consent,
    scopes::{CALENDAR_EVENTS, GMAIL_MODIFY},
};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

struct MockTokenEndpoint {
    status: u16,
    body: Value,
    seen: Mutex<Vec<HashMap<String, String>>>,
}

impl MockTokenEndpoint {
    fn ok(body: Value) -> Arc<Self> {
        Arc::new(Self {
            status: 200,
            body,
            seen: Mutex::default(),
        })
    }

    fn fail(status: u16, body: Value) -> Arc<Self> {
        Arc::new(Self {
            status,
            body,
            seen: Mutex::default(),
        })
    }

    fn forms(&self) -> Vec<HashMap<String, String>> {
        self.seen.lock().unwrap().clone()
    }
}

async fn token_handler(
    State(mock): State<Arc<MockTokenEndpoint>>,
    Form(form): Form<HashMap<String, String>>,
) -> impl IntoResponse {
    mock.seen.lock().unwrap().push(form);
    let status = StatusCode::from_u16(mock.status).unwrap();
    (status, Json(mock.body.clone()))
}

async fn serve(mock: Arc<MockTokenEndpoint>) -> String {
    let app = Router::new()
        .route("/token", post(token_handler))
        .with_state(mock);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}/token")
}

fn test_secrets() -> ClientSecrets {
    ClientSecrets {
        client_id: "client-1.apps.googleusercontent.com".to_owned(),
        client_secret: "shhh".to_owned(),
    }
}

fn seeded_store(dir: &TempDir, scopes: &[&str]) -> TokenStore {
    let store = TokenStore::at(dir.path().join("token.json"));
    store
        .save(&StoredToken {
            refresh_token: "1//refresh".to_owned(),
            scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),
        })
        .unwrap();
    store
}

#[tokio::test]
async fn access_token_is_cached_within_its_expiry_window() {
    let mock = MockTokenEndpoint::ok(json!({ "access_token": "at-1", "expires_in": 3600 }));
    let endpoint = serve(Arc::clone(&mock)).await;
    let dir = TempDir::new().unwrap();
    let auth = Authenticator::new(
        test_secrets(),
        seeded_store(&dir, &[CALENDAR_EVENTS]),
        &[CALENDAR_EVENTS],
    )
    .unwrap()
    .with_token_endpoint(endpoint);

    let first = auth.access_token().await.unwrap();
    let second = auth.access_token().await.unwrap();
    assert_eq!(first, "at-1");
    assert_eq!(second, "at-1");
    assert_eq!(
        mock.forms().len(),
        1,
        "two access-token reads inside the expiry window must share one refresh"
    );
}

#[tokio::test]
async fn access_token_refreshes_after_expiry() {
    // expires_in=70 minus the 60-second refresh margin leaves a 10-second
    // window; sleeping a little past it forces the cache to remint.
    let mock = MockTokenEndpoint::ok(json!({ "access_token": "at-1", "expires_in": 70 }));
    let endpoint = serve(Arc::clone(&mock)).await;
    let dir = TempDir::new().unwrap();
    let auth = Authenticator::new(
        test_secrets(),
        seeded_store(&dir, &[CALENDAR_EVENTS]),
        &[CALENDAR_EVENTS],
    )
    .unwrap()
    .with_token_endpoint(endpoint);

    tokio::time::pause();
    let _ = auth.access_token().await.unwrap();
    tokio::time::advance(Duration::from_secs(15)).await;
    let _ = auth.access_token().await.unwrap();
    assert_eq!(
        mock.forms().len(),
        2,
        "the second read after the cached deadline must trigger a fresh refresh"
    );
}

#[tokio::test]
async fn missing_scope_refuses_before_calling_the_token_endpoint() {
    let mock = MockTokenEndpoint::ok(json!({ "access_token": "at-1", "expires_in": 3600 }));
    let endpoint = serve(Arc::clone(&mock)).await;
    let dir = TempDir::new().unwrap();
    let auth = Authenticator::new(
        test_secrets(),
        seeded_store(&dir, &[CALENDAR_EVENTS]),
        &[CALENDAR_EVENTS, GMAIL_MODIFY],
    )
    .unwrap()
    .with_token_endpoint(endpoint);

    let err = auth.access_token().await.unwrap_err();
    let Error::ScopeMissing { missing } = &err else {
        panic!("expected Error::ScopeMissing, got {err:?}");
    };
    assert_eq!(missing, GMAIL_MODIFY);
    assert!(
        mock.forms().is_empty(),
        "the scope check must precede any token-endpoint call"
    );
}

#[tokio::test]
async fn rotated_refresh_token_replaces_the_stored_one() {
    let mock = MockTokenEndpoint::ok(json!({
        "access_token": "at-1",
        "refresh_token": "1//rotated",
        "expires_in": 3600,
    }));
    let endpoint = serve(Arc::clone(&mock)).await;
    let dir = TempDir::new().unwrap();
    let store = seeded_store(&dir, &[CALENDAR_EVENTS]);
    let auth = Authenticator::new(test_secrets(), store.clone(), &[CALENDAR_EVENTS])
        .unwrap()
        .with_token_endpoint(endpoint);

    let _ = auth.access_token().await.unwrap();

    assert_eq!(
        store.load().unwrap().refresh_token,
        "1//rotated",
        "a rotated refresh token must replace the stored one immediately"
    );
}

#[tokio::test]
async fn revoked_grant_surfaces_a_typed_error_with_the_fix() {
    let mock = MockTokenEndpoint::fail(
        400,
        json!({ "error": "invalid_grant", "error_description": "Token has been revoked." }),
    );
    let endpoint = serve(Arc::clone(&mock)).await;
    let dir = TempDir::new().unwrap();
    let auth = Authenticator::new(
        test_secrets(),
        seeded_store(&dir, &[CALENDAR_EVENTS]),
        &[CALENDAR_EVENTS],
    )
    .unwrap()
    .with_token_endpoint(endpoint);

    let err = auth.access_token().await.unwrap_err();
    assert!(matches!(err, Error::TokenRevoked), "got {err:?}");
    let message = err.to_string();
    assert!(
        message.contains("gmail auth") || message.contains("gcal auth"),
        "message must name the fix: {message}"
    );
}

#[tokio::test]
async fn load_migrates_a_legacy_path_into_the_canonical_one() {
    // The pre-extraction calendar crate wrote to <config>/gcal/token.json.
    // A user who ran `gcal auth` before this change still has only that
    // file; load() must adopt it transparently and persist into the new
    // path so subsequent refresh-rotations land there too.
    let dir = TempDir::new().unwrap();
    let legacy_path = dir.path().join("gcal").join("token.json");
    let canonical_path = dir.path().join("google").join("token.json");

    std::fs::create_dir_all(legacy_path.parent().unwrap()).unwrap();
    std::fs::write(
        &legacy_path,
        serde_json::to_vec(&StoredToken {
            refresh_token: "1//legacy".to_owned(),
            scopes: vec![CALENDAR_EVENTS.to_owned()],
        })
        .unwrap(),
    )
    .unwrap();

    let store = TokenStore::at(canonical_path.clone()).with_legacy_path(legacy_path);
    let loaded = store.load().expect("legacy file is adopted");

    assert_eq!(loaded.refresh_token, "1//legacy");
    assert!(
        canonical_path.exists(),
        "the canonical path must be populated after the migration"
    );
    let migrated = std::fs::read(&canonical_path).unwrap();
    let parsed: StoredToken = serde_json::from_slice(&migrated).unwrap();
    assert_eq!(parsed.refresh_token, "1//legacy");
}

/// Send one raw HTTP request to the consent flow's loopback listener and
/// return the raw response.
async fn send_loopback(redirect_uri: &str, path_and_query: &str) -> String {
    let url = url::Url::parse(redirect_uri).unwrap();
    let addr = format!("{}:{}", url.host_str().unwrap(), url.port().unwrap());
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(format!("GET {path_and_query} HTTP/1.1\r\nhost: x\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    response
}

#[tokio::test]
async fn consent_flow_runs_end_to_end_with_pkce() {
    let mock = MockTokenEndpoint::ok(json!({
        "access_token": "at-1",
        "refresh_token": "1//new",
        "expires_in": 3600,
        "scope": format!("{CALENDAR_EVENTS} {GMAIL_MODIFY}"),
    }));
    let endpoint = serve(Arc::clone(&mock)).await;

    let pending = begin_consent(test_secrets(), &[CALENDAR_EVENTS, GMAIL_MODIFY])
        .await
        .unwrap()
        .with_token_endpoint(endpoint);

    let auth_url = url::Url::parse(&pending.auth_url).unwrap();
    let params: HashMap<String, String> = auth_url.query_pairs().into_owned().collect();
    assert_eq!(params["response_type"], "code");
    assert_eq!(params["access_type"], "offline");
    assert_eq!(
        params["prompt"], "consent",
        "forced consent is what guarantees a refresh token"
    );
    assert_eq!(params["code_challenge_method"], "S256");
    assert_eq!(params["scope"], format!("{CALENDAR_EVENTS} {GMAIL_MODIFY}"));
    let redirect_uri = params["redirect_uri"].clone();
    let state = params["state"].clone();
    let challenge = params["code_challenge"].clone();

    // A browser-shaped peer: first a stray probe (must not end the wait),
    // then the real redirect.
    let browser = tokio::spawn(async move {
        let probe = send_loopback(&redirect_uri, "/favicon.ico").await;
        assert!(
            probe.starts_with("HTTP/1.1 404"),
            "stray requests get a 404: {probe}"
        );
        let redirect = send_loopback(&redirect_uri, &format!("/?code=code-1&state={state}")).await;
        assert!(
            redirect.contains("Authorized"),
            "the user sees a completion page: {redirect}"
        );
    });

    let code = pending.wait_loopback().await.unwrap();
    browser.await.unwrap();
    let token = pending.exchange(code).await.unwrap();
    assert_eq!(token.refresh_token, "1//new");
    assert_eq!(
        token.scopes,
        vec![CALENDAR_EVENTS.to_owned(), GMAIL_MODIFY.to_owned()],
    );

    let forms = mock.forms();
    let form = &forms[0];
    assert_eq!(form["grant_type"], "authorization_code");
    assert_eq!(form["code"], "code-1");
    assert_eq!(form["redirect_uri"], params["redirect_uri"]);
    let verifier_challenge =
        URL_SAFE_NO_PAD.encode(Sha256::digest(form["code_verifier"].as_bytes()));
    assert_eq!(
        verifier_challenge, challenge,
        "the verifier sent to the token endpoint must match the challenge in the consent URL",
    );
}

#[tokio::test]
async fn redirects_from_another_attempt_are_rejected() {
    let pending = begin_consent(test_secrets(), &[CALENDAR_EVENTS])
        .await
        .unwrap();

    let err = pending
        .code_from_redirect_url("http://127.0.0.1:1/?code=x&state=someone-elses")
        .unwrap_err();
    assert!(matches!(err, Error::StateMismatch), "got {err:?}");

    let err = pending
        .code_from_redirect_url("http://127.0.0.1:1/?error=access_denied")
        .unwrap_err();
    assert!(matches!(err, Error::ConsentDenied { .. }), "got {err:?}");

    let err = pending.code_from_redirect_url("not a url").unwrap_err();
    assert!(matches!(err, Error::RedirectParse { .. }), "got {err:?}");
}
