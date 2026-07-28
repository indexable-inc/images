//! `ClientSecrets` file loading: the bring-your-own-client path.
//!
//! Covers the shapes the Cloud Console actually emits, because getting one
//! wrong presents to an outside user as "my own OAuth client does not work"
//! with nothing pointing at the file.

use std::fs;
use std::path::Path;

use google_auth::{ClientSecrets, Error};
use tempfile::TempDir;

/// `ClientSecrets` intentionally has no `Debug` (the secret must not reach
/// logs or panic messages), which rules out `expect_err`. Unwrap by hand.
fn expect_error(path: &Path, context: &str) -> Error {
    match ClientSecrets::from_file(path) {
        Ok(_) => panic!("expected an error: {context}"),
        Err(error) => error,
    }
}

fn write(dir: &TempDir, body: &str) -> std::path::PathBuf {
    let path = dir.path().join("client_secret.json");
    fs::write(&path, body).expect("writes the fixture");
    path
}

#[test]
fn reads_a_desktop_client_as_downloaded() {
    let dir = TempDir::new().expect("temp dir");
    // Shape of the file the console hands you for an "installed" app,
    // trimmed to the keys that matter but keeping the wrapper.
    let path = write(
        &dir,
        r#"{"installed":{"client_id":"desktop-id","project_id":"p",
            "client_secret":"desktop-secret","redirect_uris":["http://localhost"]}}"#,
    );

    let secrets = ClientSecrets::from_file(&path).expect("reads the desktop client");

    assert_eq!(secrets.client_id, "desktop-id");
    assert_eq!(secrets.client_secret, "desktop-secret");
}

#[test]
fn reads_a_web_client() {
    let dir = TempDir::new().expect("temp dir");
    let path = write(
        &dir,
        r#"{"web":{"client_id":"web-id","client_secret":"web-secret"}}"#,
    );

    let secrets = ClientSecrets::from_file(&path).expect("reads the web client");

    assert_eq!(secrets.client_id, "web-id");
}

#[test]
fn reads_a_flat_object() {
    let dir = TempDir::new().expect("temp dir");
    let path = write(
        &dir,
        r#"{"client_id":"flat-id","client_secret":"flat-secret"}"#,
    );

    let secrets = ClientSecrets::from_file(&path).expect("reads the flat shape");

    assert_eq!(secrets.client_id, "flat-id");
}

#[test]
fn rejects_a_file_missing_the_secret_naming_the_path() {
    let dir = TempDir::new().expect("temp dir");
    let path = write(&dir, r#"{"installed":{"client_id":"only-an-id"}}"#);

    let error = expect_error(&path, "half a client is not a client");

    assert!(
        matches!(error, Error::ParseClientSecrets { .. }),
        "got: {error:?}"
    );
    assert!(
        error.to_string().contains(&path.display().to_string()),
        "the message must name the file so the operator knows which one to fix: {error}"
    );
}

#[test]
fn rejects_an_empty_client_id() {
    let dir = TempDir::new().expect("temp dir");
    // A truncated download, which otherwise parses and then fails much
    // later inside the consent flow with an opaque Google error.
    let path = write(&dir, r#"{"installed":{"client_id":"","client_secret":"s"}}"#);

    let error = expect_error(&path, "empty is not present");

    assert!(matches!(error, Error::ParseClientSecrets { .. }), "got: {error:?}");
}

#[test]
fn rejects_a_file_that_is_not_json() {
    let dir = TempDir::new().expect("temp dir");
    let path = write(&dir, "definitely not json");

    let error = expect_error(&path, "rejects non-JSON");

    assert!(matches!(error, Error::ParseClientSecrets { .. }), "got: {error:?}");
}

#[test]
fn reports_a_missing_file_as_a_read_error() {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("absent.json");

    let error = expect_error(&path, "no such file");

    assert!(matches!(error, Error::ReadClientSecrets { .. }), "got: {error:?}");
}
