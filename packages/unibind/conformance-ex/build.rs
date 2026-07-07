//! Emit the macOS-only `-undefined dynamic_lookup` link flag NIF libraries
//! need: the `enif_*` symbols stay undefined until the BEAM loads the
//! library, and macOS `ld` rejects undefined symbols in a dylib while Linux
//! allows them. Scoped to the cdylib via the single-colon
//! `rustc-cdylib-link-arg` directive that both `cargo` and `nix-cargo-unit`
//! honor; mirrors packages/unibind/conformance/build.rs (pyo3, same
//! mechanism).
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-cdylib-link-arg=-undefined");
        println!("cargo:rustc-cdylib-link-arg=dynamic_lookup");
    }
}
