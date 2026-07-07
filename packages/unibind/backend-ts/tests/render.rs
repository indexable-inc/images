//! Snapshot the lowering and the napi render for the sample module. The
//! committed snapshots are the review surface for what the macro generates;
//! on drift the test prints the new content to copy over the snapshot file.
//! (trybuild/macrotest would invoke cargo at test runtime, which the nix
//! test sandbox cannot do, so the render output is snapshotted directly.)

use std::fmt::Write as _;

use proc_macro2::TokenStream;
use unibind_core::ir;

fn interface() -> ir::Interface {
    let file: syn::File =
        syn::parse_str(include_str!("fixtures/sample.rs")).expect("fixture parses");
    let Some(syn::Item::Mod(module)) = file.items.first() else {
        panic!("fixture starts with a module");
    };
    unibind_core::lower_module(TokenStream::new(), module).expect("fixture lowers")
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
fn napi_glue_snapshot() {
    let interface = interface();
    let rendered = unibind_backend_ts::render(&interface).expect("renders");

    let mut shown = String::new();
    for (record, attrs) in interface.records.iter().zip(&rendered.records) {
        let outer = &attrs.outer;
        writeln!(shown, "// struct {}: {}", record.name, quote::quote!(#(#outer)*))
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
    assert_snapshot(&shown, include_str!("snapshots/sample.ts.rs"), "sample.ts.rs");
}

/// The ts backend names its unsupported surface instead of miscompiling.
#[test]
fn unsupported_surface_is_named() {
    for (source, needle) in [
        (
            "mod m { pub fn go(count: u64) {} }",
            "BigInt",
        ),
        (
            "mod m { use std::collections::HashMap; pub fn go(map: HashMap<u32, bool>) {} }",
            "non-string keys",
        ),
        (
            "mod m { #[unibind::record] pub struct R { pub size: usize } }",
            "BigInt",
        ),
    ] {
        let file: syn::File = syn::parse_str(source).expect("fixture parses");
        let Some(syn::Item::Mod(module)) = file.items.first() else {
            panic!("fixture starts with a module");
        };
        let interface =
            unibind_core::lower_module(TokenStream::new(), module).expect("fixture lowers");
        let ::std::result::Result::Err(error) = unibind_backend_ts::render(&interface) else {
            panic!("ts render accepts unsupported surface");
        };
        assert!(error.message.contains(needle), "{}", error.message);
    }
}
