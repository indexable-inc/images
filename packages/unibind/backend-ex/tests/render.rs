//! Snapshot the lowering, the rustler render, and the Elixir host files
//! for the sample module. The committed snapshots are the review surface
//! for what the macro generates; on drift the test prints the new content
//! to copy over the snapshot file. (trybuild/macrotest would invoke cargo
//! at test runtime, which the nix test sandbox cannot do, so the render
//! output is snapshotted directly.)

use std::fmt::Write as _;

use proc_macro2::TokenStream;
use unibind_core::ir;

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

    let mut shown = String::new();
    for (record, attrs) in interface.records.iter().zip(&rendered.records) {
        let outer = &attrs.outer;
        writeln!(
            shown,
            "// struct {}: {}",
            record.name,
            quote::quote!(#(#outer)*)
        )
        .expect("write to string");
        for (field, field_attrs) in record.fields.iter().zip(&attrs.fields) {
            writeln!(
                shown,
                "//   field {}: {}",
                field.name,
                quote::quote!(#(#field_attrs)*)
            )
            .expect("write to string");
        }
    }
    shown.push('\n');
    let glue: syn::File = syn::parse2(rendered.glue).expect("glue parses");
    shown.push_str(&prettyplease::unparse(&glue));
    assert_snapshot(
        &shown,
        include_str!("snapshots/sample.ex.rs"),
        "sample.ex.rs",
    );
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
