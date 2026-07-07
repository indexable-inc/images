//! Snapshot the Elixir host files for a representative interface. The
//! committed snapshots are the review surface for what `unibind-gen ex`
//! writes; on drift the test prints the new content to copy over the
//! snapshot file. The interface is built literally (every IR field is
//! `pub`) so the fixture exercises renames, docs, defaults, blocking,
//! async, streams, and objects without lowering Rust source.

use unibind_core::ir;
use unibind_gen::ex::ExEmitter;
use unibind_gen::host::HostEmitter as _;

fn names(ex: Option<&str>) -> ir::Names {
    ir::Names {
        py: None,
        ts: None,
        ex: ex.map(str::to_owned),
    }
}

fn docs(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|line| (*line).to_owned()).collect()
}

fn arg(name: &str, ty: ir::Type, default: Option<ir::Literal>) -> ir::Arg {
    ir::Arg {
        name: name.to_owned(),
        names: names(None),
        ty,
        default,
    }
}

fn function(name: &str, doc_lines: &[&str], args: Vec<ir::Arg>) -> ir::Function {
    ir::Function {
        name: name.to_owned(),
        names: names(None),
        docs: docs(doc_lines),
        asyncness: ir::Asyncness::Sync,
        blocking: false,
        args,
        ret: None,
        throws: None,
    }
}

const fn owned_string() -> ir::Type {
    ir::Type::String { owned: true }
}

fn sample_functions() -> Vec<ir::Function> {
    let rows = ir::Function {
        ret: Some(ir::Type::Vec(Box::new(ir::Type::Named("Row".to_owned())))),
        throws: Some("SampleError".to_owned()),
        ..function(
            "rows",
            &["Fetch rows.", "", "Docs become `@doc`s."],
            vec![
                arg("store", ir::Type::String { owned: false }, None),
                arg(
                    "limit",
                    ir::Type::Int(ir::IntKind::Usize),
                    Some(ir::Literal::Int(10)),
                ),
                arg(
                    "root",
                    ir::Type::Option(Box::new(ir::Type::String { owned: false })),
                    None,
                ),
            ],
        )
    };
    let recount = ir::Function {
        blocking: true,
        ret: Some(ir::Type::Int(ir::IntKind::U64)),
        ..function(
            "recount",
            &["Recount everything; long-running, so scheduled dirty."],
            vec![arg("home", ir::Type::Path { owned: true }, None)],
        )
    };
    let label = ir::Function {
        names: names(Some("label_of")),
        asyncness: ir::Asyncness::Async,
        ret: Some(owned_string()),
        ..function(
            "label",
            &["Resolve a label off the scheduler."],
            vec![arg("id", ir::Type::Int(ir::IntKind::U64), None)],
        )
    };
    let store = ir::Function {
        asyncness: ir::Asyncness::Async,
        throws: Some("SampleError".to_owned()),
        ..function(
            "store",
            &["Persist a row."],
            vec![arg("row", ir::Type::Named("Row".to_owned()), None)],
        )
    };
    let tags = ir::Function {
        ret: Some(ir::Type::Stream(Box::new(owned_string()))),
        ..function(
            "tags",
            &["Every tag, on demand."],
            vec![arg("prefix", ir::Type::String { owned: false }, None)],
        )
    };
    let scan = ir::Function {
        ret: Some(ir::Type::Stream(Box::new(ir::Type::Named(
            "Row".to_owned(),
        )))),
        throws: Some("SampleError".to_owned()),
        ..function(
            "scan",
            &["Stream rows, verifying the store first."],
            vec![arg("store", ir::Type::String { owned: false }, None)],
        )
    };
    vec![rows, recount, label, store, tags, scan]
}

fn sample_interface() -> ir::Interface {
    let row = ir::Record {
        name: "Row".to_owned(),
        names: names(Some("Line")),
        docs: docs(&["A row."]),
        fields: vec![
            ir::Field {
                name: "id".to_owned(),
                names: names(None),
                docs: docs(&["Identifier."]),
                ty: ir::Type::Int(ir::IntKind::U64),
            },
            ir::Field {
                name: "home".to_owned(),
                names: names(None),
                docs: docs(&[]),
                ty: ir::Type::Option(Box::new(ir::Type::Path { owned: true })),
            },
        ],
    };
    let error = ir::ErrorType {
        name: "SampleError".to_owned(),
        names: names(Some("SampleFault")),
        docs: docs(&["Boundary failures."]),
        py_base: None,
        variants: vec![
            ir::ErrorVariant {
                name: "StoreGone".to_owned(),
                names: names(Some("MissingStore")),
                docs: docs(&["The store is gone."]),
            },
            ir::ErrorVariant {
                name: "Invalid".to_owned(),
                names: names(None),
                docs: docs(&["Bad input."]),
            },
        ],
    };
    let cursor = ir::Object {
        name: "Cursor".to_owned(),
        names: names(None),
        docs: docs(&["A live cursor."]),
        resource: false,
        constructor: Some(ir::Function {
            throws: Some("SampleError".to_owned()),
            ..function(
                "open",
                &["Open at the start."],
                vec![arg("store", ir::Type::String { owned: false }, None)],
            )
        }),
        methods: vec![ir::Function {
            ret: Some(ir::Type::Int(ir::IntKind::U64)),
            ..function("position", &["The current position."], Vec::new())
        }],
    };
    ir::Interface {
        version: ir::IR_VERSION,
        name: "_sample".to_owned(),
        names: names(None),
        docs: docs(&["A sample boundary for the emitter tests."]),
        functions: sample_functions(),
        records: vec![row],
        enums: vec![],
        errors: vec![error],
        objects: vec![cursor],
    }
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

#[test]
fn ex_host_files_snapshot() {
    let emitter = ExEmitter {
        nif_soname: "libsample.so".to_owned(),
    };
    let files = emitter.emit(&sample_interface()).expect("emits");
    let paths: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, ["lib/sample/native.ex", "lib/sample.ex"]);
    assert_snapshot(
        &files[0].contents,
        include_str!("snapshots/sample.native.ex"),
        "sample.native.ex",
    );
    assert_snapshot(
        &files[1].contents,
        include_str!("snapshots/sample.wrapper.ex"),
        "sample.wrapper.ex",
    );
}

#[test]
fn ex_rejects_what_the_glue_rejects() {
    let mut interface = sample_interface();
    interface.functions[0].args[0].ty = ir::Type::Bytes { owned: false };
    let emitter = ExEmitter {
        nif_soname: "libsample.so".to_owned(),
    };
    let Err(error) = emitter.emit(&interface) else {
        panic!("bytes are rejected");
    };
    assert!(
        error.message.contains("binary payloads"),
        "{}",
        error.message
    );
}
