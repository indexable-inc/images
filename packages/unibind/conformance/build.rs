//! Emit the macOS-only `-undefined dynamic_lookup` link flag that `PyO3`
//! extension modules need: macOS rejects undefined symbols in a dylib, while
//! Linux allows them. Scoped to the cdylib via the single-colon
//! `rustc-cdylib-link-arg` directive that both `cargo` and `nix-cargo-unit`
//! honor. Mirrors astlog-py / search-py; scipql-py instead rides the
//! registry's `pyExtension` marker, which only covers nix workspace
//! builds, while the conformance loop also links via plain `cargo build`.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-cdylib-link-arg=-undefined");
        println!("cargo:rustc-cdylib-link-arg=dynamic_lookup");
    }
}
