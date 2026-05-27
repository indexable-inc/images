fn main() {
    // tango-bench's comparison harness loads benchmark symbols at runtime,
    // which requires the bench binary to export them. The standard incantation
    // (also used by tests/fixtures/cargo-unit-hello) is to ask cargo to link
    // benches with `-rdynamic`.
    println!("cargo:rustc-link-arg-benches=-rdynamic");
    println!("cargo:rerun-if-changed=build.rs");
}
