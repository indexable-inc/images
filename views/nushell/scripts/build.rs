fn cargo_cfg_feature() -> String {
    std::env::var("CARGO_CFG_FEATURE").unwrap_or_else(|_| {
        let mut features = std::env::vars()
            .filter_map(|(name, value)| {
                (value == "1")
                    .then_some(name)
                    .and_then(|name| name.strip_prefix("CARGO_FEATURE_").map(str::to_owned))
            })
            .map(|name| name.to_ascii_lowercase().replace('_', "-"))
            .collect::<Vec<_>>();
        features.sort_unstable();
        features.join(",")
    })
}

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").expect("set by cargo");
    println!("cargo:rerun-if-changed=scripts/build.rs");
    println!("cargo:rustc-env=NU_FEATURES={}", cargo_cfg_feature());

    if target_os == "windows" {
        println!("cargo:rerun-if-changed=assets/nu_logo.ico");
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/nu_logo.ico");
        res.compile()
            .expect("Failed to run the Windows resource compiler (rc.exe)");
    } else {
        // Tango uses dynamic linking, to allow us to dynamically change between two bench suit at runtime.
        // This is currently not supported on non nightly rust, on windows.
        println!("cargo:rustc-link-arg-benches=-rdynamic");
    }
}
