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

/// The binary codec, case by case: what to render, and the glue fragments the
/// rendering must contain paired with what their absence would mean.
///
/// One table rather than one test per module: every case asserts the same
/// property (a byte-carrying value crosses as the wire newtype and is converted
/// back at the call site) and differs only in these two columns.
const BINARY_CODEC_CASES: &[(&str, &str, &[(&str, &str)])] = &[
    (
        "an argument takes the wire newtype and borrows back out",
        "mod m { pub fn write(data: &[u8]) -> Vec<u8> { data.to_vec() } }",
        &[
            (
                "data: ::unibind_ex_runtime::Bytes",
                "argument keeps rustler's list codec",
            ),
            ("write(&data.0)", "argument does not borrow back out"),
            (
                "::unibind_ex_runtime::Bytes(super::m::write(&data.0))",
                "return is not re-wrapped",
            ),
        ],
    ),
    (
        "nested binaries convert element-wise",
        "mod m { pub fn blobs(all: Vec<Vec<u8>>) -> Option<Vec<u8>> { let _ = all; None } }",
        &[
            (
                "all: ::std::vec::Vec<::unibind_ex_runtime::Bytes>",
                "nested argument keeps rustler's list codec",
            ),
            (
                "all.into_iter().map(|value| value.0).collect()",
                "nested argument is not unwrapped element-wise",
            ),
            (
                ".map(|value| ::unibind_ex_runtime::Bytes(value))",
                "nested return is not re-wrapped element-wise",
            ),
        ],
    ),
    (
        "stream items are re-wrapped",
        "mod m { pub fn blobs() -> UniStream<Vec<u8>> { unimplemented!() } }",
        &[(
            "::unibind_ex_runtime::map_stream(",
            "stream items are not re-wrapped",
        )],
    ),
];

#[test]
fn binaries_cross_as_the_wire_newtype() {
    for (case, source, expected) in BINARY_CODEC_CASES {
        let interface = lower_module_source(source);
        let rendered = unibind_backend_ex::render(&interface, None).expect("renders");
        let glue = prettyplease::unparse(&syn::parse2(rendered.glue).expect("glue parses"));
        for (fragment, why) in *expected {
            assert!(glue.contains(fragment), "{case}: {why}: {glue}");
        }
    }
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
