fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // Eyra replaces both libc and the C startup code; -nostartfiles drops
    // crt0 so Eyra's Rust `origin` runtime provides the program entry point.
    if std::env::var_os("CARGO_FEATURE_EYRA").is_some() {
        println!("cargo:rustc-link-arg=-nostartfiles");
    }
}
