//! Snapshot the lowering and the three JVM renders for the sample module.
//! The committed snapshots are the review surface for what the macro and
//! the generators produce; on drift the test prints the new content to copy
//! over the snapshot file. (trybuild/macrotest would invoke cargo at test
//! runtime, which the nix test sandbox cannot do, so the render output is
//! snapshotted directly.)

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

/// One expected generated file.
struct Expected {
    path: &'static str,
    snapshot: &'static str,
    name: &'static str,
}

#[test]
fn ir_json_snapshot() {
    let json = serde_json::to_string_pretty(&interface()).expect("serializes");
    assert_snapshot(&json, include_str!("snapshots/sample.ir.json"), "sample.ir.json");
}

#[test]
fn rust_glue_snapshot() {
    let rendered = unibind_backend_jvm::render(&interface()).expect("renders");
    let glue: syn::File = syn::parse2(rendered.glue).expect("glue parses");
    assert_snapshot(
        &prettyplease::unparse(&glue),
        include_str!("snapshots/sample.jvm.rs"),
        "sample.jvm.rs",
    );
}

#[test]
fn java_snapshot() {
    let files = unibind_backend_jvm::generate_java(&interface()).expect("generates");
    let expected = [
        Expected {
            path: "unibind/sample/Sample.java",
            snapshot: include_str!("snapshots/sample.Sample.java"),
            name: "sample.Sample.java",
        },
        Expected {
            path: "unibind/sample/Row.java",
            snapshot: include_str!("snapshots/sample.Row.java"),
            name: "sample.Row.java",
        },
        Expected {
            path: "unibind/sample/SampleErrorException.java",
            snapshot: include_str!("snapshots/sample.SampleErrorException.java"),
            name: "sample.SampleErrorException.java",
        },
        Expected {
            path: "unibind/sample/UnibindPanicException.java",
            snapshot: include_str!("snapshots/sample.UnibindPanicException.java"),
            name: "sample.UnibindPanicException.java",
        },
    ];
    let actual_paths: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
    let expected_paths: Vec<&str> = expected.iter().map(|entry| entry.path).collect();
    assert_eq!(actual_paths, expected_paths);
    for (file, entry) in files.iter().zip(&expected) {
        assert_snapshot(&file.content, entry.snapshot, entry.name);
    }
}

#[test]
fn kotlin_snapshot() {
    let files = unibind_backend_jvm::generate_kotlin(&interface()).expect("generates");
    let actual_paths: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(actual_paths, ["unibind/sample/Sample.kt"]);
    assert_snapshot(
        &files[0].content,
        include_str!("snapshots/sample.Sample.kt"),
        "sample.Sample.kt",
    );
}

#[test]
fn async_functions_are_rejected() {
    let mut interface = interface();
    interface.functions[0].asyncness = ir::Asyncness::Async;
    let error = unibind_backend_jvm::render(&interface).expect_err("async must not render");
    assert!(
        error.message.contains("issue #1992"),
        "missing phase pointer: {}",
        error.message
    );
}

#[test]
fn data_enums_are_rejected() {
    let mut interface = interface();
    interface.enums.push(ir::Enum {
        name: "Kind".to_owned(),
        names: ir::Names::default(),
        docs: Vec::new(),
        variants: Vec::new(),
    });
    let error = unibind_backend_jvm::generate_java(&interface).expect_err("enums must not render");
    assert!(
        error.message.contains("data enum"),
        "missing enum rejection: {}",
        error.message
    );
}
