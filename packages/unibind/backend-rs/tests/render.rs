//! Snapshot the lowering, the engine glue, and every generated client file
//! for the sample module. The committed snapshots are the review surface
//! for what the backend generates; on drift the test prints the new content
//! to copy over the snapshot file. (trybuild/macrotest would invoke cargo at
//! test runtime, which the nix test sandbox cannot do, so the render output
//! is snapshotted directly; the conformance crates compile-and-run the same
//! shapes for real.) The Cargo.toml and package.nix snapshots carry a
//! `.snap` suffix so filename-keyed tooling (the package registry walks
//! every `package.nix`) never mistakes them for real metadata.

use proc_macro2::TokenStream;
use unibind_core::ir;

fn lower(source: &str) -> ir::Interface {
    let file: syn::File = syn::parse_str(source).expect("fixture parses");
    let Some(syn::Item::Mod(module)) = file.items.first() else {
        panic!("fixture starts with a module");
    };
    unibind_core::lower_module(TokenStream::new(), module).expect("fixture lowers")
}

fn interface() -> ir::Interface {
    lower(include_str!("fixtures/sample.rs"))
}

fn assert_snapshot(actual: &str, expected: &str, name: &str) {
    if actual.trim() == expected.trim() {
        return;
    }
    println!("=== actual {name} ===");
    println!("{actual}");
    println!("=== end {name} ===");
    panic!("{name} drifted; copy the printed block into tests/snapshots/{name}");
}

#[test]
fn ir_json_snapshot() {
    let json = serde_json::to_string_pretty(&interface()).expect("serializes");
    assert_snapshot(&json, include_str!("snapshots/sample.ir.json"), "sample.ir.json");
}

#[test]
fn engine_glue_snapshot() {
    let rendered = unibind_backend_rs::render(&interface()).expect("renders");
    let glue: syn::File = syn::parse2(rendered.glue).expect("glue parses");
    assert_snapshot(
        &prettyplease::unparse(&glue),
        include_str!("snapshots/sample.engine.rs"),
        "sample.engine.rs",
    );
}

fn client() -> unibind_backend_rs::RenderedCrate {
    unibind_backend_rs::render_client(
        &interface(),
        &unibind_backend_rs::ClientOptions {
            crate_name: "sample-client".to_owned(),
            workspace_deps: true,
        },
    )
    .expect("client renders")
}

fn assert_client_file(path: &str, expected: &str, name: &str) {
    let rendered = client();
    let file = rendered
        .files
        .iter()
        .find(|file| file.path == path)
        .unwrap_or_else(|| panic!("client renders {path}"));
    assert_snapshot(&file.contents, expected, name);
}

#[test]
fn client_renders_exactly_the_expected_files() {
    let rendered = client();
    let paths: Vec<&str> = rendered.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "Cargo.toml",
            "src/lib.rs",
            "src/records.rs",
            "src/abi.rs",
            "src/error.rs",
            "src/engine.rs",
            "package.nix",
        ]
    );
}

#[test]
fn client_cargo_toml_snapshot() {
    assert_client_file(
        "Cargo.toml",
        include_str!("snapshots/client/Cargo.toml.snap"),
        "client/Cargo.toml",
    );
}

#[test]
fn client_package_nix_snapshot() {
    assert_client_file(
        "package.nix",
        include_str!("snapshots/client/package.nix.snap"),
        "client/package.nix",
    );
}

#[test]
fn client_lib_snapshot() {
    assert_client_file(
        "src/lib.rs",
        include_str!("snapshots/client/lib.rs"),
        "client/lib.rs",
    );
}

#[test]
fn client_records_snapshot() {
    assert_client_file(
        "src/records.rs",
        include_str!("snapshots/client/records.rs"),
        "client/records.rs",
    );
}

#[test]
fn client_abi_snapshot() {
    assert_client_file(
        "src/abi.rs",
        include_str!("snapshots/client/abi.rs"),
        "client/abi.rs",
    );
}

#[test]
fn client_error_snapshot() {
    assert_client_file(
        "src/error.rs",
        include_str!("snapshots/client/error.rs"),
        "client/error.rs",
    );
}

#[test]
fn client_engine_snapshot() {
    assert_client_file(
        "src/engine.rs",
        include_str!("snapshots/client/engine.rs"),
        "client/engine.rs",
    );
}

#[test]
fn handshake_hash_matches_the_embed_bytes() {
    // The handshake hashes the exact link-section bytes; recompute from
    // `embed::ir_json` and expect the same hex in the rendered glue.
    let interface = interface();
    let json = unibind_core::embed::ir_json(&interface).expect("serializes");
    let expected = {
        use sha2::Digest as _;
        hex::encode(sha2::Sha256::digest(&json))
    };
    let glue = unibind_backend_rs::render(&interface).expect("renders").glue.to_string();
    assert!(glue.contains(&expected), "glue embeds the IR hash");
    let engine = client()
        .files
        .into_iter()
        .find(|file| file.path == "src/engine.rs")
        .expect("client renders src/engine.rs");
    assert!(engine.contents.contains(&expected), "client bakes the same hash");
}

#[test]
fn data_enums_are_rejected() {
    let interface = lower("mod m { }");
    let mut with_enum = interface;
    with_enum.enums.push(ir::Enum {
        name: "Kind".to_owned(),
        names: ir::Names::default(),
        docs: Vec::new(),
        variants: Vec::new(),
    });
    let error = unibind_backend_rs::render(&with_enum).expect_err("enums are phase 2");
    assert!(error.message.contains("data enum"), "{}", error.message);
}
