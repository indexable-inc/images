//! Read a Chromium app's `Safe Storage` secret from the macOS login Keychain.
//!
//! There is no stable one-shot C API for a generic-password read that respects
//! the access-control prompt, so we shell out to `/usr/bin/security`, the path
//! Chrome itself documents. The first read of another app's secret pops the
//! standard Keychain dialog; `Always Allow` makes it silent thereafter.

use anyhow::{Result, bail};
use std::process::Command;

/// Fetch the cleartext password for the generic-password service `service`
/// (e.g. `"Dia Safe Storage"`), the raw bytes Chromium feeds to `PBKDF2`.
pub fn safe_storage_password(service: &str) -> Result<Vec<u8>> {
    let out = Command::new("/usr/bin/security")
        .args(["find-generic-password", "-w", "-s", service])
        .output()?;

    if !out.status.success() {
        let status = out.status;
        let msg = String::from_utf8_lossy(&out.stderr);
        bail!("security exited {status}: {}", msg.trim());
    }

    // `-w` prints the password followed by a trailing newline.
    let mut pw = out.stdout;
    if pw.last() == Some(&b'\n') {
        pw.pop();
    }
    if pw.is_empty() {
        bail!("empty password for Keychain service {service:?}");
    }
    Ok(pw)
}
