//! SRI (`sha256-<base64>`) hash construction for the fetcher `hash` slots.

use anyhow::{Context, Result, ensure};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

/// SRI string from a hex-encoded sha256 digest (`PyPI` digests, upstream
/// release manifests).
pub fn from_hex(hex_digest: &str) -> Result<String> {
    let bytes = hex::decode(hex_digest.trim())
        .with_context(|| format!("invalid hex sha256 digest `{hex_digest}`"))?;
    ensure!(
        bytes.len() == 32,
        "hex sha256 digest `{hex_digest}` is {} bytes, expected 32",
        bytes.len()
    );
    Ok(format!("sha256-{}", STANDARD.encode(bytes)))
}

/// SRI string from a nix base32 sha256 (`nix-prefetch-url` output), via
/// `nix hash convert` so nix's quirky base32 alphabet stays nix's problem.
pub fn from_nix_base32(base32: &str) -> Result<String> {
    let output = std::process::Command::new("nix")
        .args(["hash", "convert", "--hash-algo", "sha256", "--to", "sri"])
        .arg(base32.trim())
        .output()
        .context("failed to run `nix hash convert`")?;
    ensure!(
        output.status.success(),
        "`nix hash convert` failed for `{base32}`:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::from_hex;

    #[test]
    fn hex_digest_round_trips_to_sri() {
        // sha256("") in hex vs its well-known SRI form.
        let hex = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert_eq!(
            from_hex(hex).unwrap(),
            "sha256-47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU="
        );
    }

    #[test]
    fn short_or_malformed_hex_is_rejected() {
        assert!(from_hex("abcd").is_err());
        assert!(from_hex("not hex").is_err());
    }
}
