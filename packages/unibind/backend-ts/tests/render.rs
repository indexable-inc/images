//! Snapshot the lowering and the napi render for the sample module. The
//! committed snapshots are the review surface for what the macro generates;
//! on drift the test prints the new content to copy over the snapshot file.
//! (trybuild/macrotest would invoke cargo at test runtime, which the nix
//! test sandbox cannot do, so the render output is snapshotted directly.)

use unibind_core::ir;
use unibind_test_support::{assert_ir_json_snapshot, assert_render_snapshot, lower_module_source};

const GLUE_SNAPSHOT: &str = include_str!("snapshots/sample.ts.rs");

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
fn napi_glue_snapshot() {
    let interface = interface();
    let rendered = unibind_backend_ts::render(&interface).expect("renders");

    assert_render_snapshot!(interface, rendered, GLUE_SNAPSHOT, "sample.ts.rs");
}

/// Every stream-returning export gets its own handle class, scoped by its
/// owner: the fixture's free `tail` and `Counter::tail` are both streams,
/// and one class named for both would silently misbind them.
#[test]
fn stream_classes_are_owner_scoped() {
    let glue = unibind_backend_ts::render(&interface())
        .expect("renders")
        .glue
        .to_string();
    for class in [
        "__UnibindStreamTail",
        "__UnibindStreamTailLater",
        "__UnibindStreamCounterWatch",
        "__UnibindStreamCounterTail",
    ] {
        // The declaration, not the bare name: `__UnibindStreamTail` is a
        // prefix of `__UnibindStreamTailLater`, so a bare `contains` would
        // pass on the wrong class.
        let declaration = format!("pub struct {class} {{");
        assert!(
            glue.contains(&declaration),
            "`{class}` is missing from the glue"
        );
    }
}

/// The ts backend names its unsupported surface instead of miscompiling.
#[test]
fn unsupported_surface_is_named() {
    for (source, needle) in [
        ("mod m { pub fn go(count: u64) {} }", "BigInt"),
        (
            "mod m { use std::collections::HashMap; pub fn go(map: HashMap<u32, bool>) {} }",
            "non-string keys",
        ),
        (
            "mod m { #[unibind::record] pub struct R { pub size: usize } }",
            "BigInt",
        ),
    ] {
        let interface = lower_module_source(source);
        let ::std::result::Result::Err(error) = unibind_backend_ts::render(&interface) else {
            panic!("ts render accepts unsupported surface");
        };
        assert!(error.message.contains(needle), "{}", error.message);
    }
}
