//! Emit the macOS-only `-undefined dynamic_lookup` link flag that `PyO3`
//! extension modules need: macOS rejects undefined symbols in a dylib, while
//! Linux allows them. `pyo3` does not emit it and the shared cargo-unit graph
//! does not add it, so emit it here, scoped to the cdylib via the
//! single-colon `rustc-cdylib-link-arg` directive that both `cargo` and
//! `nix-cargo-unit` honor. This mirrors search-py's and tui-py's build.rs.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-cdylib-link-arg=-undefined");
        println!("cargo:rustc-cdylib-link-arg=dynamic_lookup");
    }
}
