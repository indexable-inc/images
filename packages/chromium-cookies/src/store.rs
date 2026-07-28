//! Read and decrypt rows from a Chromium `Cookies` `SQLite` database.
//!
//! The DB is opened read-only and `immutable=1` so a running browser holding a
//! write-ahead-log lock does not block us. Each row's `encrypted_value` is
//! decrypted with the caller's key; a row that fails to decrypt surfaces the
//! error in its value rather than being dropped.

use crate::crypto;
use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use std::path::Path;

/// One decrypted cookie, with the fields a sync target needs to replant it.
#[derive(Debug, Clone, Serialize)]
pub struct Cookie {
    pub host: String,
    pub name: String,
    pub value: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: i64,
    /// Chromium epoch: microseconds since 1601-01-01 UTC. 0 means a session cookie.
    pub expires_utc: i64,
}

/// Read every cookie from `db`, decrypting values with `key`.
///
/// # Errors
/// Fails when the database cannot be opened read-only or a row cannot be read.
pub fn read(db: &Path, key: &[u8; 16]) -> Result<Vec<Cookie>> {
    let uri = format!("file:{}?immutable=1", db.display());
    let conn = Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .with_context(|| format!("opening {}", db.display()))?;

    let mut stmt = conn.prepare(
        "SELECT host_key, name, value, encrypted_value, path, \
                is_secure, is_httponly, samesite, expires_utc \
         FROM cookies",
    )?;

    let rows = stmt.query_map([], |r| {
        let plain: String = r.get(2)?;
        let enc: Vec<u8> = r.get(3)?;
        let value = decrypt_field(&plain, &enc, key);

        Ok(Cookie {
            host: r.get(0)?,
            name: r.get(1)?,
            value,
            path: r.get(4)?,
            secure: r.get::<_, i64>(5)? != 0,
            http_only: r.get::<_, i64>(6)? != 0,
            same_site: r.get(7)?,
            expires_utc: r.get(8)?,
        })
    })?;

    rows.collect::<rusqlite::Result<Vec<Cookie>>>()
        .context("reading cookie rows")
}

/// Prefer a plaintext `value`; otherwise decrypt `encrypted_value`, reporting a
/// failure inline so one bad row never sinks the whole export.
fn decrypt_field(plain: &str, enc: &[u8], key: &[u8; 16]) -> String {
    if !plain.is_empty() {
        return plain.to_owned();
    }
    if enc.is_empty() {
        return String::new();
    }
    match crypto::decrypt(enc, key) {
        Ok(pt) => crypto::decode_value(&pt),
        Err(e) => format!("<decrypt failed: {e}>"),
    }
}
