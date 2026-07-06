//! Snapshot the lowering and the pyo3 render for the sample module. The
//! committed snapshots are the review surface for what the macro generates;
//! on drift the test prints the new content to copy over the snapshot file.
//! (trybuild/macrotest would invoke cargo at test runtime, which the nix
//! test sandbox cannot do, so the render output is snapshotted directly.)

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
fn pyo3_glue_snapshot() {
    let interface = interface();
    let rendered = unibind_backend_py::render(&interface).expect("renders");

    let mut shown = String::new();
    for (record, attrs) in interface.records.iter().zip(&rendered.records) {
        let outer = &attrs.outer;
        shown.push_str(&format!(
            "// struct {}: {}\n",
            record.name,
            quote::quote!(#(#outer)*)
        ));
        for (field, field_attrs) in record.fields.iter().zip(&attrs.fields) {
            shown.push_str(&format!(
                "//   field {}: {}\n",
                field.name,
                quote::quote!(#(#field_attrs)*)
            ));
        }
    }
    shown.push('\n');
    let glue: syn::File = syn::parse2(rendered.glue).expect("glue parses");
    shown.push_str(&prettyplease::unparse(&glue));
    assert_snapshot(&shown, include_str!("snapshots/sample.py.rs"), "sample.py.rs");
}
