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

/// The ts backend names its unsupported surface instead of miscompiling.
#[test]
fn unsupported_surface_is_named() {
    for (source, needle) in [
        (
            "mod m { use std::collections::HashMap; pub fn go(map: HashMap<u32, bool>) {} }",
            "non-string keys",
        ),
        (
            "mod m { use std::collections::HashMap; \
             #[unibind::record] pub struct R { pub m: HashMap<u32, bool> } }",
            "non-string keys",
        ),
    ] {
        let interface = lower_module_source(source);
        let ::std::result::Result::Err(error) = unibind_backend_ts::render(&interface) else {
            panic!("ts render accepts unsupported surface");
        };
        assert!(error.message.contains(needle), "{}", error.message);
    }
}

/// Every 64-bit width renders, in both directions, as napi's `BigInt`:
/// nothing on this surface is rejected any more, and nothing on it is
/// declared as a plain Rust integer the boundary would truncate.
#[test]
fn wide_integers_render_as_bigint() {
    for source in [
        "mod m { pub fn go(count: u64) {} }",
        "mod m { pub fn go() -> usize { 0 } }",
        "mod m { pub fn go(offset: isize) -> i64 { 0 } }",
        "mod m { pub fn go(counts: Vec<u64>) -> Option<i64> { None } }",
        "mod m { #[unibind::record] pub struct R { pub size: usize } }",
    ] {
        let interface = lower_module_source(source);
        let rendered = unibind_backend_ts::render(&interface).expect("renders");
        let glue = rendered.glue.to_string();
        assert!(glue.contains("BigInt"), "{glue}");
        for width in ["u64", "usize", "isize"] {
            assert!(
                !glue.contains(&format!(": {width}")),
                "{width} reached a signature: {glue}"
            );
        }
    }
}
