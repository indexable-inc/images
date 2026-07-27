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

/// A fragment the rendered glue must contain, and what its absence would mean.
struct Fragment {
    text: &'static str,
    absence_means: &'static str,
}

/// One module to render, and the fragments its glue must carry.
struct BinaryCodecCase {
    name: &'static str,
    source: &'static str,
    expected: &'static [Fragment],
}

/// The binary codec, case by case: what to render, and the glue fragments the
/// rendering must contain paired with what their absence would mean.
///
/// One table rather than one test per module: every case asserts the same
/// property (a byte-carrying value crosses as the wire newtype and is converted
/// back at the call site) and differs only in these two columns.
const BINARY_CODEC_CASES: &[BinaryCodecCase] = &[
    BinaryCodecCase {
        name: "an argument takes the wire newtype and borrows back out",
        source: "mod m { pub fn write(data: &[u8]) -> Vec<u8> { data.to_vec() } }",
        expected: &[
            Fragment {
                text: "data: ::unibind_ex_runtime::Bytes",
                absence_means: "argument keeps rustler's list codec",
            },
            Fragment {
                text: "write(&data.0)",
                absence_means: "argument does not borrow back out",
            },
            Fragment {
                text: "::unibind_ex_runtime::Bytes(super::m::write(&data.0))",
                absence_means: "return is not re-wrapped",
            },
        ],
    },
    BinaryCodecCase {
        name: "nested binaries convert element-wise",
        source: "mod m { pub fn blobs(all: Vec<Vec<u8>>) -> Option<Vec<u8>> { let _ = all; None } }",
        expected: &[
            Fragment {
                text: "all: ::std::vec::Vec<::unibind_ex_runtime::Bytes>",
                absence_means: "nested argument keeps rustler's list codec",
            },
            Fragment {
                text: "all.into_iter().map(|value| value.0).collect()",
                absence_means: "nested argument is not unwrapped element-wise",
            },
            Fragment {
                text: ".map(|value| ::unibind_ex_runtime::Bytes(value))",
                absence_means: "nested return is not re-wrapped element-wise",
            },
        ],
    },
    BinaryCodecCase {
        name: "stream items are re-wrapped",
        source: "mod m { pub fn blobs() -> UniStream<Vec<u8>> { unimplemented!() } }",
        expected: &[Fragment {
            text: "::unibind_ex_runtime::map_stream(",
            absence_means: "stream items are not re-wrapped",
        }],
    },
];

#[test]
fn binaries_cross_as_the_wire_newtype() {
    for case in BINARY_CODEC_CASES {
        let interface = lower_module_source(case.source);
        let rendered = unibind_backend_ex::render(&interface, None).expect("renders");
        let glue = prettyplease::unparse(&syn::parse2(rendered.glue).expect("glue parses"));
        let missing: Vec<&str> = case
            .expected
            .iter()
            .filter(|fragment| !glue.contains(fragment.text))
            .map(|fragment| fragment.absence_means)
            .collect();
        assert!(missing.is_empty(), "{}: {missing:?}: {glue}", case.name);
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
