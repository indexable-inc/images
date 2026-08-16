//! The one literal-IR sample interface that the host-emitter snapshot tests
//! render.
//!
//! The two emitter tests (gen/tests/ts.rs and gen/tests/wasm.rs) diff their
//! snapshot sets against each other: the targets publish one surface, so every
//! difference between the sets is meant to be a policy that one of those files
//! states a rule for. That reading is sound only while both render the
//! IDENTICAL fixture, and two copies cannot guarantee it -- an edit to one copy
//! turns a fixture artifact into what reads as a real divergence. So the shapes
//! live here once, and each test supplies only its own name and doc line.

use unibind_core::ir;

use crate::fixtures::{self, arg, docs};

#[must_use]
pub fn names(py: Option<&str>, ts: Option<&str>) -> ir::Names {
    ir::Names {
        py: py.map(str::to_owned),
        ts: ts.map(str::to_owned),
        ..ir::Names::default()
    }
}

#[must_use]
pub fn field(name: &str, ts: Option<&str>, doc_lines: &[&str], ty: ir::Type) -> ir::Field {
    ir::Field {
        name: name.to_owned(),
        names: names(None, ts),
        docs: docs(doc_lines),
        ty,
    }
}

#[must_use]
pub fn function(name: &str, ts: Option<&str>, doc_lines: &[&str], args: Vec<ir::Arg>) -> ir::Function {
    fixtures::function(name, names(None, ts), doc_lines, args)
}

#[must_use]
pub const fn owned_string() -> ir::Type {
    ir::Type::String { owned: true }
}

#[must_use]
pub fn named(name: &str) -> ir::Type {
    ir::Type::Named(name.to_owned())
}

#[must_use]
pub fn functions() -> Vec<ir::Function> {
    let rows = ir::Function {
        ret: Some(ir::Type::Vec(Box::new(named("Row")))),
        throws: Some("SampleError".to_owned()),
        ..function(
            "rows",
            None,
            &["Fetch rows.", "", "Docs reach the generated `.d.ts`."],
            vec![
                arg("store", ir::Type::String { owned: false }, None),
                arg(
                    "limit",
                    ir::Type::Int(ir::IntKind::U32),
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
    let touch = ir::Function {
        ret: Some(ir::Type::Bool),
        ..function(
            "touch",
            Some("touchPath"),
            &[],
            vec![
                arg("path", ir::Type::Path { owned: false }, None),
                arg("data", ir::Type::Bytes { owned: false }, None),
                arg(
                    "ratio",
                    ir::Type::Float(ir::FloatKind::F64),
                    Some(ir::Literal::Float(0.5)),
                ),
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
        ..function(
            "fetch",
            None,
            &["Fetch one row."],
            vec![arg("store", owned_string(), None)],
        )
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
            vec![arg(
                "start",
                ir::Type::Int(ir::IntKind::I64),
                Some(ir::Literal::Int(0)),
            )],
        )
    };
    vec![rows, touch, slow_add, fetch, tail, tail_later, open_counter]
}

#[must_use]
pub fn records() -> Vec<ir::Record> {
    vec![ir::Record {
        name: "Row".to_owned(),
        names: names(None, Some("SampleRow")),
        docs: docs(&["A row."]),
        fields: vec![
            field(
                "id",
                None,
                &["Identifier."],
                ir::Type::Int(ir::IntKind::I64),
            ),
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
            // The two byte positions, side by side. Each backend states
            // its own rule for them in its own snapshots; the fixture only
            // guarantees that both positions are present.
            field("blob", None, &[], ir::Type::Bytes { owned: true }),
            field(
                "chunks",
                None,
                &[],
                ir::Type::Vec(Box::new(ir::Type::Bytes { owned: true })),
            ),
            field(
                "home",
                None,
                &[],
                ir::Type::Option(Box::new(ir::Type::Path { owned: true })),
            ),
        ],
    }]
}

#[must_use]
pub fn errors() -> Vec<ir::ErrorType> {
    vec![ir::ErrorType {
        name: "SampleError".to_owned(),
        names: names(None, None),
        docs: docs(&["Boundary failures."]),
        py_base: None,
        jvm_base: None,
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

#[must_use]
pub fn objects() -> Vec<ir::Object> {
    let constructor = ir::Function {
        throws: Some("SampleError".to_owned()),
        ..function(
            "new",
            None,
            &["Open a counter."],
            vec![arg(
                "start",
                ir::Type::Int(ir::IntKind::I64),
                Some(ir::Literal::Int(0)),
            )],
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
    let watch = ir::Function {
        ret: Some(ir::Type::Stream(Box::new(ir::Type::Int(ir::IntKind::I64)))),
        ..function(
            "watch",
            None,
            &["Every value the counter takes."],
            Vec::new(),
        )
    };
    let tail = ir::Function {
        asyncness: ir::Asyncness::Async,
        ret: Some(ir::Type::Stream(Box::new(owned_string()))),
        throws: Some("SampleError".to_owned()),
        ..function(
            "tail",
            Some("tailRows"),
            &["Labels under `prefix` (async, throwing, renamed)."],
            vec![
                arg("prefix", owned_string(), None),
                arg(
                    "limit",
                    ir::Type::Int(ir::IntKind::U32),
                    Some(ir::Literal::Int(10)),
                ),
            ],
        )
    };
    let fork = ir::Function {
        ret: Some(named("Counter")),
        ..function("fork", None, &["Fork a counter."], Vec::new())
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
        associated: Vec::new(),
        methods: vec![value, add, watch, tail, fork, close],
    }]
}

/// The sample interface, under a target-specific name and doc line.
#[must_use]
pub fn interface(name: &str, doc: &str) -> ir::Interface {
    ir::Interface {
        version: ir::IR_VERSION,
        name: name.to_owned(),
        names: names(None, None),
        docs: docs(&[doc]),
        functions: functions(),
        records: records(),
        enums: vec![],
        errors: errors(),
        objects: objects(),
    }
}
