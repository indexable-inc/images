//! Snapshot the swift-bridge render for the sample module. The committed
//! snapshots are the review surface for what the macro generates and what
//! `unibind-gen swift` writes; on drift the test prints the new content to
//! copy over the snapshot file. (trybuild/macrotest would invoke cargo at
//! test runtime, which the nix test sandbox cannot do, so the render output
//! is snapshotted directly.)

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

fn unparsed(tokens: &TokenStream) -> String {
    let file: syn::File = syn::parse2(tokens.clone()).expect("rendered tokens parse");
    prettyplease::unparse(&file)
}

#[test]
fn bridge_module_snapshot() {
    let rendered = unibind_backend_swift::render(&interface()).expect("renders");
    assert_snapshot(
        &unparsed(&rendered.bridge),
        include_str!("snapshots/sample.bridge.rs"),
        "sample.bridge.rs",
    );
}

#[test]
fn glue_snapshot() {
    let rendered = unibind_backend_swift::render(&interface()).expect("renders");
    assert_snapshot(
        &unparsed(&rendered.glue),
        include_str!("snapshots/sample.glue.rs"),
        "sample.glue.rs",
    );
}

#[test]
fn overlay_snapshot() {
    let rendered = unibind_backend_swift::render(&interface()).expect("renders");
    assert_snapshot(
        &rendered.overlay,
        include_str!("snapshots/sample.swift"),
        "sample.swift",
    );
}

#[test]
fn objects_are_rejected_with_a_phase_pointer() {
    let mut sketch = interface();
    sketch.objects.push(ir::Object {
        name: "Store".to_owned(),
        names: ir::Names::default(),
        docs: Vec::new(),
        resource: false,
        constructor: None,
        methods: Vec::new(),
    });
    let error = unibind_backend_swift::render(&sketch).expect_err("objects must not render");
    assert!(error.message.contains("issue #2082"), "{}", error.message);
}

#[test]
fn data_enums_are_rejected() {
    let mut sketch = interface();
    sketch.enums.push(ir::Enum {
        name: "Kind".to_owned(),
        names: ir::Names::default(),
        docs: Vec::new(),
        variants: Vec::new(),
    });
    let error = unibind_backend_swift::render(&sketch).expect_err("enums must not render");
    assert!(error.message.contains("does not render"), "{}", error.message);
}

#[test]
fn async_functions_are_rejected_with_a_phase_pointer() {
    let mut sketch = interface();
    sketch.functions[0].asyncness = ir::Asyncness::Async;
    let error = unibind_backend_swift::render(&sketch).expect_err("async must not render");
    assert!(error.message.contains("phase 2"), "{}", error.message);
}
