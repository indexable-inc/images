//! Snapshot the TypeScript host files for a representative interface. The
//! committed snapshots are the review surface for what `unibind-gen ts`
//! writes; on drift the test prints the new content to copy over the
//! snapshot file. The interface is built literally (every IR field is
//! `pub`) so the fixture covers renames, docs, defaults, the async
//! cancellation surface, streams, and objects without lowering Rust source.

use unibind_core::ir;
use unibind_gen::host::HostEmitter as _;
use unibind_gen::ts::TsEmitter;

fn names(py: Option<&str>, ts: Option<&str>) -> ir::Names {
    ir::Names {
        py: py.map(str::to_owned),
        ts: ts.map(str::to_owned),
        ex: None,
    }
}

fn docs(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|line| (*line).to_owned()).collect()
}

fn arg(name: &str, ty: ir::Type, default: Option<ir::Literal>) -> ir::Arg {
    ir::Arg {
        name: name.to_owned(),
        names: names(None, None),
        ty,
        default,
    }
}

fn field(name: &str, ts: Option<&str>, doc_lines: &[&str], ty: ir::Type) -> ir::Field {
    ir::Field {
        name: name.to_owned(),
        names: names(None, ts),
        docs: docs(doc_lines),
        ty,
    }
}

fn function(name: &str, ts: Option<&str>, doc_lines: &[&str], args: Vec<ir::Arg>) -> ir::Function {
    ir::Function {
        name: name.to_owned(),
        names: names(None, ts),
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

fn named(name: &str) -> ir::Type {
    ir::Type::Named(name.to_owned())
}

fn sample_functions() -> Vec<ir::Function> {
    let rows = ir::Function {
        ret: Some(ir::Type::Vec(Box::new(named("Row")))),
        throws: Some("SampleError".to_owned()),
        ..function(
            "rows",
            None,
            &["Fetch rows.", "", "Docs reach the generated `.d.ts`."],
            vec![
                arg("store", ir::Type::String { owned: false }, None),
                arg("limit", ir::Type::Int(ir::IntKind::U32), Some(ir::Literal::Int(10))),
                arg("root", ir::Type::Option(Box::new(ir::Type::String { owned: false })), None),
            ],
        )
    };
    let touch = ir::Function {
        ret: Some(ir::Type::Bool),
        ..function(
            "touch",
            Some("touchPath"),
            &[],
            vec![
                arg("path", ir::Type::Path { owned: false }, None),
                arg("data", ir::Type::Bytes { owned: false }, None),
                arg("ratio", ir::Type::Float(ir::FloatKind::F64), Some(ir::Literal::Float(0.5))),
            ],
        )
    };
    let slow_add = ir::Function {
        asyncness: ir::Asyncness::Async,
        ret: Some(ir::Type::Int(ir::IntKind::I64)),
        ..function(
            "slow_add",
            None,
            &["Add, slowly."],
            vec![
                arg("a", ir::Type::Int(ir::IntKind::I64), None),
                arg("b", ir::Type::Int(ir::IntKind::I64), None),
            ],
        )
    };
    let fetch = ir::Function {
        asyncness: ir::Asyncness::Async,
        ret: Some(named("Row")),
        throws: Some("SampleError".to_owned()),
        ..function("fetch", None, &["Fetch one row."], vec![arg("store", owned_string(), None)])
    };
    let tail = ir::Function {
        ret: Some(ir::Type::Stream(Box::new(named("Row")))),
        ..function(
            "tail",
            None,
            &["Tail rows as a pull stream."],
            vec![arg("store", ir::Type::String { owned: false }, None)],
        )
    };
    let tail_later = ir::Function {
        asyncness: ir::Asyncness::Async,
        ret: Some(ir::Type::Stream(Box::new(named("Row")))),
        throws: Some("SampleError".to_owned()),
        ..function(
            "tail_later",
            None,
            &["Tail rows once the store opens."],
            vec![arg("store", owned_string(), None)],
        )
    };
    let open_counter = ir::Function {
        ret: Some(named("Counter")),
        ..function(
            "open_counter",
            None,
            &["Open a counter from a free function."],
            vec![arg("start", ir::Type::Int(ir::IntKind::I64), Some(ir::Literal::Int(0)))],
        )
    };
    vec![rows, touch, slow_add, fetch, tail, tail_later, open_counter]
}

fn sample_records() -> Vec<ir::Record> {
    vec![ir::Record {
        name: "Row".to_owned(),
        names: names(None, Some("SampleRow")),
        docs: docs(&["A row."]),
        fields: vec![
            field("id", None, &["Identifier."], ir::Type::Int(ir::IntKind::I64)),
            field("name", Some("rowLabel"), &[], owned_string()),
            field("tags", None, &[], ir::Type::Vec(Box::new(owned_string()))),
            field(
                "weights",
                None,
                &[],
                ir::Type::Map {
                    key: Box::new(owned_string()),
                    value: Box::new(ir::Type::Float(ir::FloatKind::F64)),
                },
            ),
            field("blob", None, &[], ir::Type::Bytes { owned: true }),
            field("home", None, &[], ir::Type::Option(Box::new(ir::Type::Path { owned: true }))),
        ],
    }]
}

fn sample_errors() -> Vec<ir::ErrorType> {
    vec![ir::ErrorType {
        name: "SampleError".to_owned(),
        names: names(None, None),
        docs: docs(&["Boundary failures."]),
        py_base: None,
        variants: vec![
            ir::ErrorVariant {
                name: "StoreGone".to_owned(),
                names: names(None, Some("StoreGoneError")),
                docs: docs(&["The store is gone."]),
            },
            ir::ErrorVariant {
                name: "Invalid".to_owned(),
                names: names(None, None),
                docs: docs(&["Bad input."]),
            },
        ],
    }]
}

fn sample_objects() -> Vec<ir::Object> {
    let constructor = ir::Function {
        throws: Some("SampleError".to_owned()),
        ..function(
            "new",
            None,
            &["Open a counter."],
            vec![arg("start", ir::Type::Int(ir::IntKind::I64), Some(ir::Literal::Int(0)))],
        )
    };
    let value = ir::Function {
        ret: Some(ir::Type::Int(ir::IntKind::I64)),
        ..function("value", None, &["Current value."], Vec::new())
    };
    let add = ir::Function {
        asyncness: ir::Asyncness::Async,
        ret: Some(ir::Type::Int(ir::IntKind::I64)),
        throws: Some("SampleError".to_owned()),
        ..function(
            "add",
            Some("addSlowly"),
            &["Add and return the new value."],
            vec![arg("amount", ir::Type::Int(ir::IntKind::I64), None)],
        )
    };
    let close = ir::Function {
        asyncness: ir::Asyncness::Async,
        ..function("close", None, &["Release the counter."], Vec::new())
    };
    vec![ir::Object {
        name: "Counter".to_owned(),
        names: names(None, None),
        docs: docs(&["A counter resource."]),
        resource: true,
        constructor: Some(constructor),
        methods: vec![value, add, close],
    }]
}

fn interface() -> ir::Interface {
    ir::Interface {
        version: ir::IR_VERSION,
        name: "sample_ts".to_owned(),
        names: names(None, None),
        docs: docs(&["A sample boundary exercising the ts surface."]),
        functions: sample_functions(),
        records: sample_records(),
        enums: vec![],
        errors: sample_errors(),
        objects: sample_objects(),
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
fn ts_host_files_snapshot() {
    let emitter = TsEmitter {
        addon: "sample_ts".to_owned(),
    };
    let files = emitter.emit(&interface()).expect("emits");
    let paths: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, ["index.d.ts", "index.js"]);
    assert_snapshot(&files[0].contents, include_str!("snapshots/sample.d.ts"), "sample.d.ts");
    assert_snapshot(&files[1].contents, include_str!("snapshots/sample.js"), "sample.js");
}

#[test]
fn bigint_only_integers_are_rejected() {
    let mut bad = interface();
    bad.functions.push(ir::Function {
        ret: Some(ir::Type::Int(ir::IntKind::U64)),
        ..function("total", None, &[], Vec::new())
    });
    let emitter = TsEmitter {
        addon: "sample_ts".to_owned(),
    };
    let error = emitter.emit(&bad).expect_err("u64 must not emit");
    assert!(error.message.contains("BigInt"), "{}", error.message);
}
