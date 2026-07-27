//! Snapshot the lowering, the rustler render, and the Elixir host files
//! for the sample module. The committed snapshots are the review surface
//! for what the macro generates; on drift the test prints the new content
//! to copy over the snapshot file. (trybuild/macrotest would invoke cargo
//! at test runtime, which the nix test sandbox cannot do, so the render
//! output is snapshotted directly.)

use unibind_core::ir;
use unibind_test_support::{assert_ir_json_snapshot, assert_render_snapshot, lower_module_source};

const GLUE_SNAPSHOT: &str = include_str!("snapshots/sample.ex.rs");

fn interface() -> ir::Interface {
    lower_module_source(include_str!("fixtures/sample.rs"))
}

#[test]
fn ir_json_snapshot() {
    assert_ir_json_snapshot(
        &interface(),
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

/// Lower and render `source`, returning the rejection message; the
/// rejection tests only vary in their input and the message they expect.
fn render_failure(source: &str) -> String {
    let interface = lower_module_source(source);
    match unibind_backend_ex::render(&interface, None) {
        Ok(_) => panic!("ex render accepts unsupported surface: {source}"),
        Err(error) => error.message,
    }
}

#[test]
fn async_stream_functions_are_rejected() {
    let message =
        render_failure("mod m { pub async fn feed() -> UniStream<u64> { unimplemented!() } }");
    assert!(message.contains("plain fn"), "{message}");
}

/// Lower and render `source`, returning the rendered glue as text.
fn render_glue(source: &str) -> String {
    let interface = lower_module_source(source);
    let rendered = unibind_backend_ex::render(&interface, None).expect("renders");
    prettyplease::unparse(&syn::parse2(rendered.glue).expect("glue parses"))
}

#[test]
fn binary_arguments_take_the_wire_newtype_and_borrow_back_out() {
    let glue = render_glue("mod m { pub fn write(data: &[u8]) -> Vec<u8> { data.to_vec() } }");
    assert!(
        glue.contains("data: ::unibind_ex_runtime::Bytes"),
        "argument keeps rustler's list codec: {glue}"
    );
    assert!(glue.contains("write(&data.0)"), "{glue}");
    assert!(
        glue.contains("::unibind_ex_runtime::Bytes(super::m::write(&data.0))"),
        "return is not re-wrapped: {glue}"
    );
}

#[test]
fn nested_binaries_convert_element_wise() {
    let glue = render_glue(
        "mod m { pub fn blobs(all: Vec<Vec<u8>>) -> Option<Vec<u8>> { let _ = all; None } }",
    );
    assert!(
        glue.contains("all: ::std::vec::Vec<::unibind_ex_runtime::Bytes>"),
        "{glue}"
    );
    assert!(
        glue.contains("all.into_iter().map(|value| value.0).collect()"),
        "{glue}"
    );
    assert!(
        glue.contains(".map(|value| ::unibind_ex_runtime::Bytes(value))"),
        "{glue}"
    );
}

#[test]
fn binary_stream_items_are_re_wrapped() {
    let glue = render_glue(
        "mod m { pub fn blobs() -> UniStream<Vec<u8>> { unimplemented!() } }",
    );
    assert!(
        glue.contains("::unibind_ex_runtime::map_stream("),
        "{glue}"
    );
}

#[test]
fn binary_record_fields_are_rejected() {
    let message = render_failure(
        "mod m { #[unibind::record] #[derive(Clone)] pub struct R { pub blob: Vec<u8> } }",
    );
    assert!(message.contains("carries binary data"), "{message}");
}

#[test]
fn field_ex_renames_are_rejected() {
    let message = render_failure(
        "mod m { #[unibind::record] #[derive(Clone)] pub struct R { \
         #[unibind(ex(name = \"tag\"))] pub name: String } }",
    );
    assert!(message.contains("rename the Rust field"), "{message}");
}
