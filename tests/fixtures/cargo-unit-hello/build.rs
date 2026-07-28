fn main() {
    println!("cargo:rustc-link-arg-benches=-rdynamic");
    println!("cargo:rerun-if-changed=build.rs");

    // Round-trips `packageBuildEnv` so a test can prove the build script saw it:
    // read the build-time env var and re-expose it to the crate as a compile-time
    // env var. Defaults to "absent" so the crate compiles when nothing is set.
    println!("cargo:rerun-if-env-changed=CARGO_UNIT_BUILD_ENV");
    let build_env = std::env::var("CARGO_UNIT_BUILD_ENV").unwrap_or_else(|_| "absent".to_owned());
    println!("cargo:rustc-env=CARGO_UNIT_HELLO_BUILD_ENV={build_env}");
}
