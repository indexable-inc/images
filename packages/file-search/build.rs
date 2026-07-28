fn main() {
    // tango-bench's comparison harness loads benchmark symbols at runtime,
    // which requires the bench binary to export them. Linux's `ld` takes
    // `-rdynamic`; on macOS the linker equivalent is `-Wl,-export_dynamic`.
    // Other targets get no flag — tango-bench just won't expose the
    // comparison path there, which is fine because the workspace only
    // builds benches on x86_64-linux today.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let flag = match target_os.as_str() {
        "linux" => Some("-rdynamic"),
        "macos" => Some("-Wl,-export_dynamic"),
        _ => None,
    };
    if let Some(flag) = flag {
        println!("cargo:rustc-link-arg-benches={flag}");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
