//! Snapshot the lowering, the rustler render, and the Elixir host files
//! for the sample module. The committed snapshots are the review surface
//! for what the macro generates; on drift the test prints the new content
//! to copy over the snapshot file. (trybuild/macrotest would invoke cargo
//! at test runtime, which the nix test sandbox cannot do, so the render
//! output is snapshotted directly.)

use proc_macro2::TokenStream;
use unibind_core::ir;
use unibind_test_support::{assert_render_snapshot, assert_snapshot};

const GLUE_SNAPSHOT: &str = include_str!("snapshots/sample.ex.rs");

fn lower(source: &str) -> ir::Interface {
    let file: syn::File = syn::parse_str(source).expect("module parses");
    let Some(syn::Item::Mod(module)) = file.items.first() else {
        panic!("source starts with a module");
    };
    unibind_core::lower_module(TokenStream::new(), module).expect("module lowers")
}

fn interface() -> ir::Interface {
    lower(include_str!("fixtures/sample.rs"))
}

#[test]
fn ir_json_snapshot() {
    let json = serde_json::to_string_pretty(&interface()).expect("serializes");
    assert_snapshot(
        &json,
        include_str!("snapshots/sample.ir.json"),
        "sample.ir.json",
    );
}

#[test]
fn rustler_glue_snapshot() {
    let interface = interface();
    let rendered = unibind_backend_ex::render(&interface, Some("sample")).expect("renders");

    assert_render_snapshot!(interface, rendered, GLUE_SNAPSHOT, "sample.ex.rs");
}

#[test]
fn async_stream_functions_are_rejected() {
    let interface = lower(
        "mod m { pub async fn feed() -> UniStream<u64> { \
         unimplemented!() } }",
    );
    let Err(error) = unibind_backend_ex::render(&interface, None) else {
        panic!("async streams are rejected");
    };
    assert!(
        error.message.contains("plain fn"),
        "{}",
        error.message
    );
}

#[test]
fn binary_payloads_are_rejected() {
    let interface = lower("mod m { pub fn write(data: &[u8]) {} }");
    let Err(error) = unibind_backend_ex::render(&interface, None) else {
        panic!("bytes are rejected");
    };
    assert!(
        error.message.contains("binary payloads"),
        "{}",
        error.message
    );
}

#[test]
fn field_ex_renames_are_rejected() {
    let interface = lower(
        "mod m { #[unibind::record] #[derive(Clone)] pub struct R { \
         #[unibind(ex(name = \"tag\"))] pub name: String } }",
    );
    let Err(error) = unibind_backend_ex::render(&interface, None) else {
        panic!("field renames are rejected");
    };
    assert!(
        error.message.contains("rename the Rust field"),
        "{}",
        error.message
    );
}
