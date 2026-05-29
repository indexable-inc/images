//! PyO3 extension modules resolve interpreter symbols at load time rather than
//! at link time. macOS rejects undefined symbols in a dylib by default, so the
//! cdylib link needs `-undefined dynamic_lookup` (Linux permits them, so it
//! needs nothing). pyo3 0.28 does not emit this and the shared cargo-unit graph
//! does not add it, so emit it here, scoped to the cdylib via the single-colon
//! `rustc-cdylib-link-arg` directive that both cargo and nix-cargo-unit honor
//! (the latter reads exactly this form; see nix-cargo-unit render.rs). This
//! mirrors what `napi-build` does for the tui-node addon.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-cdylib-link-arg=-undefined");
        println!("cargo:rustc-cdylib-link-arg=dynamic_lookup");
    }
}
