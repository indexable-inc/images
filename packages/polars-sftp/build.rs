//! macOS rejects undefined symbols in a dylib while Linux allows them, and a
//! PyO3 extension module leaves the CPython symbols undefined (resolved at import
//! time). Emit `-undefined dynamic_lookup` on macOS, scoped to the cdylib. Same
//! shape as the repo's other PyO3 crates (search-py/tui-py).
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-cdylib-link-arg=-undefined");
        println!("cargo:rustc-cdylib-link-arg=dynamic_lookup");
    }
}
