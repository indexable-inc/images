//! Snapshot the TypeScript host files for a representative interface. The
//! committed snapshots are the review surface for what `unibind-gen ts`
//! writes; on drift the test prints the new content to copy over the
//! snapshot file. The interface is built literally (every IR field is
//! `pub`) so the fixture covers renames, docs, defaults, the async
//! cancellation surface, streams, and objects without lowering Rust source.

use unibind_core::ir;
use unibind_gen::host::HostEmitter as _;
use unibind_gen::ts::TsEmitter;
use unibind_test_support::assert_host_snapshots;
use unibind_test_support::fixtures::{self, arg, docs};

fn names(py: Option<&str>, ts: Option<&str>) -> ir::Names {
    ir::Names {
        py: py.map(str::to_owned),
        ts: ts.map(str::to_owned),
        ..ir::Names::default()
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
    fixtures::function(name, names(None, ts), doc_lines, args)
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

fn sample_records() -> Vec<ir::Record> {
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
            field("blob", None, &[], ir::Type::Bytes { owned: true }),
            field(
                "home",
                None,
                &[],
                ir::Type::Option(Box::new(ir::Type::Path { owned: true })),
            ),
        ],
    }]
}

fn sample_errors() -> Vec<ir::ErrorType> {
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

fn sample_objects() -> Vec<ir::Object> {
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
        ..function("watch", None, &["Every value the counter takes."], Vec::new())
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
        methods: vec![value, add, watch, tail, fork, close],
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

fn emitter() -> TsEmitter {
    TsEmitter {
        addon: "sample_ts".to_owned(),
    }
}

/// The `schemas.ts` an interface emits, which every schema assertion below
/// reads instead of re-deriving the file's position in the emit order.
fn schemas(interface: &ir::Interface) -> String {
    emitter()
        .emit(interface)
        .expect("emits")
        .into_iter()
        .find(|file| file.path == "schemas.ts")
        .expect("schemas.ts is emitted")
        .contents
}

#[test]
fn ts_host_files_snapshot() {
    let files = emitter().emit(&interface()).expect("emits");
    assert_host_snapshots(
        files
            .iter()
            .map(|file| (file.path.as_str(), file.contents.as_str())),
        &[
            (
                "index.d.ts",
                "sample.d.ts",
                include_str!("snapshots/sample.d.ts"),
            ),
            (
                "schemas.ts",
                "sample.schemas.ts",
                include_str!("snapshots/sample.schemas.ts"),
            ),
            ("index.js", "sample.js", include_str!("snapshots/sample.js")),
        ],
    );
}

/// Every width past an IEEE double's exact range is `bigint` on both
/// surfaces, and only those: the narrower ones stay `number`, which is what
/// keeps the common case ergonomic. One table drives both assertions because
/// one list (`ts::types::crosses_as_bigint`) drives both renderers -- a
/// change to one of them that missed the other would leave a schema checking
/// a `number` where the declared type promises a `bigint`, and that is the
/// failure this catches.
#[test]
fn wide_integers_are_bigint_in_the_types_and_the_schemas() {
    let widths = [
        ("total", ir::IntKind::U64, "bigint", "z.bigint()"),
        ("offset", ir::IntKind::Isize, "bigint", "z.bigint()"),
        ("size", ir::IntKind::Usize, "bigint", "z.bigint()"),
        ("count", ir::IntKind::I64, "bigint", "z.bigint()"),
        ("narrow", ir::IntKind::U32, "number", "z.number().int()"),
    ];
    let mut wide = interface();
    for (name, kind, _, _) in widths {
        // Both positions: a bare return, and a record field, which is where
        // the mirror-struct half of the mapping lands.
        wide.functions.push(ir::Function {
            ret: Some(ir::Type::Int(kind)),
            ..function(name, None, &[], Vec::new())
        });
        wide.records[0]
            .fields
            .push(field(name, None, &[], ir::Type::Int(kind)));
    }

    let files = emitter().emit(&wide).expect("emits");
    let emitted = |path: &str| {
        files
            .iter()
            .find(|file| file.path == path)
            .expect("the file is emitted")
            .contents
            .clone()
    };
    let dts = emitted("index.d.ts");
    let schemas = emitted("schemas.ts");
    for (name, _, declared, schema) in widths {
        assert!(
            dts.contains(&format!("export declare function {name}(): {declared};")),
            "{dts}"
        );
        assert!(schemas.contains(&format!("{name}: {schema},")), "{schemas}");
    }
}

/// A record-less interface would get a `schemas.ts` holding nothing but an
/// unused `zod` import, so it gets no file at all.
#[test]
fn an_interface_without_records_emits_no_schemas_file() {
    let mut bare = interface();
    bare.records = Vec::new();
    bare.objects = Vec::new();
    bare.errors = Vec::new();
    bare.functions = vec![function("ping", None, &["Ping."], Vec::new())];
    let files = emitter().emit(&bare).expect("emits");
    let paths: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, ["index.d.ts", "index.js"]);
}

/// `const` bindings are not hoisted, so a record referencing one declared
/// later reads it through a thunk, and one referencing an earlier record
/// reads the binding directly.
#[test]
fn a_forward_record_reference_defers_through_lazy() {
    let head = ir::Record {
        name: "Head".to_owned(),
        names: names(None, None),
        docs: docs(&[]),
        fields: vec![field("row", None, &[], named("Row"))],
    };

    let mut forward = interface();
    forward.records.insert(0, head.clone());
    let rendered = schemas(&forward);
    assert!(
        rendered.contains("row: z.lazy(() => SampleRow),"),
        "{rendered}"
    );

    let mut backward = interface();
    backward.records.push(head);
    let rendered = schemas(&backward);
    assert!(rendered.contains("row: SampleRow,"), "{rendered}");
}

/// A cyclic record graph has no emission order that binds every schema
/// before it is read, so it refuses by name instead of landing a file that
/// cannot evaluate.
#[test]
fn a_record_reference_cycle_is_rejected() {
    let mut cyclic = interface();
    cyclic.records.push(ir::Record {
        name: "Node".to_owned(),
        names: names(None, None),
        docs: docs(&[]),
        fields: vec![field(
            "kids",
            None,
            &[],
            ir::Type::Vec(Box::new(named("Node"))),
        )],
    });
    let error = emitter().emit(&cyclic).expect_err("a cycle must not emit");
    assert!(
        error.message.contains("Node -> Node") && error.message.contains("reference cycle"),
        "{}",
        error.message
    );
}

/// An object handle crosses by reference; its fields never leave Rust, so
/// there is nothing for a schema to validate.
#[test]
fn a_record_field_naming_an_object_is_rejected() {
    let mut bad = interface();
    bad.records[0]
        .fields
        .push(field("counter", None, &[], named("Counter")));
    let error = emitter().emit(&bad).expect_err("an object has no schema");
    assert!(
        error.message.contains("is not a record"),
        "{}",
        error.message
    );
}

/// A non-resource object with no constructor: instances only ever come from
/// the export that returns them.
fn returned_object(name: &str, doc: &str, methods: Vec<ir::Function>) -> ir::Object {
    ir::Object {
        name: name.to_owned(),
        names: names(None, None),
        docs: docs(&[doc]),
        resource: false,
        constructor: None,
        methods,
    }
}

/// An object whose methods stream and hand back another object: the shapes
/// the ix SDK's `VmHandle` and `client.keys().create(...)` need. Kept off
/// the shared fixture so these tests state their rule instead of restating
/// a snapshot.
fn namespaced_interface() -> ir::Interface {
    let meta = ir::Record {
        name: "Meta".to_owned(),
        names: names(None, None),
        docs: docs(&["Row provenance."]),
        fields: vec![field("source", None, &[], owned_string())],
    };
    let row = ir::Record {
        name: "Row".to_owned(),
        names: names(None, None),
        docs: docs(&["A row composing another record two ways."]),
        fields: vec![
            field("meta", None, &[], ir::Type::Option(Box::new(named("Meta")))),
            field(
                "meta_by_key",
                None,
                &[],
                ir::Type::Map {
                    key: Box::new(owned_string()),
                    value: Box::new(named("Meta")),
                },
            ),
        ],
    };
    let create = ir::Function {
        ret: Some(owned_string()),
        ..function("create", None, &["Mint a key."], Vec::new())
    };
    let watch = ir::Function {
        ret: Some(ir::Type::Stream(Box::new(owned_string()))),
        ..function("watch", None, &["Every event, as a pull stream."], Vec::new())
    };
    let namespace = ir::Function {
        ret: Some(named("Keys")),
        ..function("keys", None, &["This client's keys namespace."], Vec::new())
    };
    ir::Interface {
        objects: vec![
            returned_object("Keys", "The keys namespace.", vec![create]),
            returned_object("Client", "A client.", vec![watch, namespace]),
        ],
        records: vec![meta, row],
        functions: Vec::new(),
        errors: Vec::new(),
        ..interface()
    }
}

/// The two host files the ts emitter writes for one interface.
struct HostFiles {
    dts: String,
    js: String,
}

/// Emit `interface` and pick the two files these tests read by name, so
/// adding an emitted file (`schemas.ts`) does not disturb them.
fn emit(interface: &ir::Interface) -> HostFiles {
    let emitter = TsEmitter {
        addon: "sample_ts".to_owned(),
    };
    let files = emitter.emit(interface).expect("emits");
    let read = |path: &str| {
        let index = files
            .iter()
            .position(|file| file.path == path)
            .unwrap_or_else(|| panic!("{path} was not emitted"));
        files[index].contents.clone()
    };
    HostFiles {
        dts: read("index.d.ts"),
        js: read("index.js"),
    }
}

/// What an object's methods hand back. A stream types as the shared
/// `UnibindStream<T>` (and pulls its declaration in even though no free
/// function streams), and an object return arrives as the wrapper class:
/// a bare native handle decodes no errors and has no disposal, so the
/// `.d.ts` would be declaring something the runtime never produced.
#[test]
fn object_methods_wrap_their_stream_and_object_returns() {
    let HostFiles { dts, js } = emit(&namespaced_interface());
    for declared in [
        "export interface UnibindStream<T> extends AsyncIterable<T>",
        "  watch(): UnibindStream<string>;",
        "  keys(): Keys;",
    ] {
        assert!(dts.contains(declared), "`{declared}` is missing:\n{dts}");
    }
    for wrapped in [
        "return wrapStream(this.#handle.watch(...args));",
        "return new Keys(nativeHandle, this.#handle.keys(...args));",
    ] {
        assert!(js.contains(wrapped), "`{wrapped}` is missing:\n{js}");
    }
}

/// Records compose: another record under `Option` (optional in both
/// directions) and as a map value.
#[test]
fn records_compose_under_option_and_as_map_values() {
    let HostFiles { dts, .. } = emit(&namespaced_interface());
    for declared in ["  meta?: Meta | null;", "  metaByKey: Record<string, Meta>;"] {
        assert!(dts.contains(declared), "`{declared}` is missing:\n{dts}");
    }
}
