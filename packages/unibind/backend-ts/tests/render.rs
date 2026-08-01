//! Snapshot the lowering and the napi render for the sample module. The
//! committed snapshots are the review surface for what the macro generates;
//! on drift the test prints the new content to copy over the snapshot file.
//! (trybuild/macrotest would invoke cargo at test runtime, which the nix
//! test sandbox cannot do, so the render output is snapshotted directly.)
//!
//! Rules that hold across interfaces -- how a class is named, what a return
//! is wrapped in -- are asserted on their own small lowered modules instead,
//! so the rule is stated where it is checked rather than left for a reader
//! to infer from a 300-line snapshot.

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

/// The rendered glue as readable Rust, for tests that assert on one item
/// rather than on the whole committed snapshot.
fn glue_source(source: &str) -> String {
    let interface = lower_module_source(source);
    let rendered = unibind_backend_ts::render(&interface).expect("renders");
    let file: syn::File = syn::parse2(rendered.glue).expect("glue parses");
    prettyplease::unparse(&file)
}

/// Every stream-returning export gets its own handle class, named for its
/// owner. A free `tail` and a `Store::tail` are both streams, and one class
/// serving both would silently misbind them.
#[test]
fn stream_classes_are_owner_scoped() {
    let glue = glue_source(
        "mod m {
            #[unibind::object]
            pub struct Store {}

            impl Store {
                pub fn tail(&self) -> unibind_runtime::UniStream<i64> {
                    todo!()
                }
            }

            pub fn tail() -> unibind_runtime::UniStream<i64> {
                todo!()
            }
        }",
    );
    for class in ["__UnibindStreamTail", "__UnibindStreamStoreTail"] {
        // The declaration, not the bare name: one class name can be a
        // prefix of another, and a bare `contains` would pass on the wrong
        // one.
        let declaration = format!("pub struct {class} {{");
        assert!(
            glue.contains(&declaration),
            "`{class}` is missing from the glue:\n{glue}"
        );
    }
    assert!(
        glue.contains("__UnibindStreamStoreTail::__unibind_from"),
        "the method does not wrap its stream in the owner-scoped class:\n{glue}"
    );
}

/// A method returning another object crosses as that object's handle
/// class, which is what makes `client.keys().create(...)` chain.
#[test]
fn a_method_returning_an_object_crosses_as_its_handle() {
    let glue = glue_source(
        "mod m {
            #[unibind::object]
            pub struct Keys {}

            impl Keys {
                pub fn create(&self) -> String {
                    todo!()
                }
            }

            #[unibind::object]
            pub struct Client {}

            impl Client {
                pub fn keys(&self) -> Keys {
                    todo!()
                }
            }
        }",
    );
    assert!(
        glue.contains("__UnibindObjectKeys::__unibind_from"),
        "`Client::keys` does not wrap its return in the Keys handle:\n{glue}"
    );
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

/// The glue reaches the user's module through exactly one alias, bound at
/// the glue module's own scope, and never writes `super::` anywhere else.
///
/// napi-derive relocates the items it expands into generated helper modules
/// (`mod __napi_helper__<name>`), and a `super::` written inside one of
/// those resolves one level short of the crate root: `cannot find <module>
/// in super`. It bit the first real adopter on every object whose
/// constructor or method mentions a user type, and an adopter cannot work
/// around it, because nothing can inject items into a generated module. The
/// alias survives the relocation, since the helper modules `use super::*`.
#[test]
fn the_glue_reaches_the_user_module_only_through_its_alias() {
    let glue = glue_source(include_str!("fixtures/sample.rs"));
    let hops: Vec<&str> = glue
        .lines()
        .map(str::trim)
        .filter(|line| line.contains("super::"))
        .collect();
    assert_eq!(
        hops,
        vec!["use super::sample_ts as __unibind_user;"],
        "the glue must spell `super::` only in its own alias binding"
    );
    assert!(
        glue.contains("__unibind_user::Row"),
        "named types must resolve through the alias:\n{glue}"
    );
}
