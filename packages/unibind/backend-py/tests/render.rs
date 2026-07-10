//! Snapshot the lowering and the pyo3 render for the sample module. The
//! committed snapshots are the review surface for what the macro generates;
//! on drift the test prints the new content to copy over the snapshot file.
//! (trybuild/macrotest would invoke cargo at test runtime, which the nix
//! test sandbox cannot do, so the render output is snapshotted directly.)

use proc_macro2::TokenStream;
use unibind_core::ir;
use unibind_test_support::{assert_render_snapshot, assert_snapshot};

const GLUE_SNAPSHOT: &str = include_str!("snapshots/sample.py.rs");

fn interface() -> ir::Interface {
    let file: syn::File =
        syn::parse_str(include_str!("fixtures/sample.rs")).expect("fixture parses");
    let Some(syn::Item::Mod(module)) = file.items.first() else {
        panic!("fixture starts with a module");
    };
    unibind_core::lower_module(TokenStream::new(), module).expect("fixture lowers")
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

    assert_render_snapshot!(interface, rendered, GLUE_SNAPSHOT, "sample.py.rs");
}
