//! Snapshot the lowering, the C-ABI glue render, and the generated Java
//! class for the sample module. The committed snapshots are the review
//! surface for what the macro generates; on drift the test prints the new
//! content to copy over the snapshot file. (trybuild/macrotest would invoke
//! cargo at test runtime, which the nix test sandbox cannot do, so the
//! render output is snapshotted directly.)

use proc_macro2::TokenStream;
use unibind_core::ir;
use unibind_test_support::{assert_render_snapshot, assert_snapshot};

const GLUE_SNAPSHOT: &str = include_str!("snapshots/sample.jvm.rs");

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
fn c_abi_glue_snapshot() {
    let interface = interface();
    let rendered = unibind_backend_jvm::render(&interface).expect("renders");

    assert_render_snapshot!(interface, rendered, GLUE_SNAPSHOT, "sample.jvm.rs");
}

#[test]
fn java_class_snapshot() {
    let host = unibind_backend_jvm::host_class(&interface(), Some("com.example.sample"))
        .expect("renders");
    assert_eq!(host.class_name, "Sample");
    assert_eq!(host.file_name, "Sample.java");
    assert_snapshot(
        &host.source,
        include_str!("snapshots/Sample.java"),
        "Sample.java",
    );
}

/// The jvm backend names its unsupported surface instead of miscompiling.
#[test]
fn unsupported_surface_is_named() {
    for (source, needle) in [
        (
            "mod m { pub async fn go() -> u64 { 0 } }",
            "blocks on a runtime",
        ),
        (
            "mod m { pub fn feed() -> UniStream<u64> { unimplemented!() } }",
            "return a `Vec<T>` instead",
        ),
        (
            "mod m { #[unibind::object] pub struct Cursor { pos: u64 } \
             impl Cursor { #[unibind(constructor)] pub fn open() -> Self { \
             Self { pos: 0 } } } }",
            "expose free functions instead",
        ),
        (
            "mod m { pub fn go(input: Option<Option<bool>>) {} }",
            "flatten the option",
        ),
        (
            "mod m { #[unibind::error(jvm(base = \"Throwable\"))] pub enum E { \
             Gone { message: String } } pub fn go() -> Result<(), E> { Ok(()) } }",
            "not a supported Java base exception",
        ),
        (
            "mod m { pub fn go(class: bool) {} }",
            "a Java keyword",
        ),
        (
            "mod m { pub fn go(status: bool) {} }",
            "the generated method bodies reserve",
        ),
        (
            "mod m { pub fn free() {} }",
            "buffer-free symbol",
        ),
    ] {
        let interface = lower(source);
        let ::std::result::Result::Err(error) = unibind_backend_jvm::render(&interface) else {
            panic!("jvm render accepts unsupported surface: {source}");
        };
        assert!(error.message.contains(needle), "{}", error.message);
    }
}
