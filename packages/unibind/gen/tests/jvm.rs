//! The Java emitter seam: the package's dots map onto directories, the
//! unnamed package lands at the output root, and the contents are exactly
//! the class `unibind-backend-jvm` renders — the same interface and
//! snapshot as that crate's render tests, so the two review surfaces
//! cannot drift apart.

use proc_macro2::TokenStream;
use unibind_core::ir;
use unibind_gen::host::HostEmitter as _;
use unibind_gen::jvm::JvmEmitter;
use unibind_test_support::assert_host_snapshots;

fn sample_interface() -> ir::Interface {
    let source = include_str!("../../backend-jvm/tests/fixtures/sample.rs");
    let file: syn::File = syn::parse_str(source).expect("module parses");
    let Some(syn::Item::Mod(module)) = file.items.first() else {
        panic!("source starts with a module");
    };
    unibind_core::lower_module(TokenStream::new(), module).expect("module lowers")
}

#[test]
fn jvm_host_file_lands_at_the_package_path() {
    let emitter = JvmEmitter {
        package: Some("com.example.sample".to_owned()),
    };
    let files = emitter.emit(&sample_interface()).expect("emits");
    assert_host_snapshots(
        files
            .iter()
            .map(|file| (file.path.as_str(), file.contents.as_str())),
        &[(
            "com/example/sample/Sample.java",
            "Sample.java",
            include_str!("../../backend-jvm/tests/snapshots/Sample.java"),
        )],
    );
}

#[test]
fn jvm_unnamed_package_lands_at_the_root() {
    let emitter = JvmEmitter { package: None };
    let files = emitter.emit(&sample_interface()).expect("emits");
    let paths: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, ["Sample.java"]);
    assert!(!files[0].contents.contains("package com.example.sample;"));
}

#[test]
fn jvm_rejects_what_the_glue_rejects() {
    let mut interface = sample_interface();
    interface.functions[0].args[0].ty =
        ir::Type::Option(Box::new(ir::Type::Option(Box::new(ir::Type::Bool))));
    let emitter = JvmEmitter { package: None };
    let Err(error) = emitter.emit(&interface) else {
        panic!("nested options are rejected");
    };
    assert!(
        error.message.contains("flatten the option"),
        "{}",
        error.message
    );
}
