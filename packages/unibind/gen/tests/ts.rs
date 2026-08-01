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

#[test]
fn ts_host_files_snapshot() {
    let emitter = TsEmitter {
        addon: "sample_ts".to_owned(),
    };
    let files = emitter.emit(&interface()).expect("emits");
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
            ("index.js", "sample.js", include_str!("snapshots/sample.js")),
        ],
    );
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

/// An object whose methods stream and hand back another object: the shapes
/// the ix SDK's `VmHandle` and `client.keys().create(...)` need, kept off
/// the shared fixture so this states the rule rather than restating a
/// snapshot.
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
        docs: docs(&["A row, composing another record two ways."]),
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
    let keys = ir::Object {
        name: "Keys".to_owned(),
        names: names(None, None),
        docs: docs(&["The keys namespace."]),
        resource: false,
        constructor: None,
        methods: vec![create],
    };
    let watch = ir::Function {
        ret: Some(ir::Type::Stream(Box::new(owned_string()))),
        ..function("watch", None, &["Every event, as a pull stream."], Vec::new())
    };
    let namespace = ir::Function {
        ret: Some(named("Keys")),
        ..function("keys", None, &["This client's keys namespace."], Vec::new())
    };
    let client = ir::Object {
        name: "Client".to_owned(),
        names: names(None, None),
        docs: docs(&["A client."]),
        resource: false,
        constructor: None,
        methods: vec![watch, namespace],
    };
    ir::Interface {
        objects: vec![keys, client],
        records: vec![meta, row],
        functions: Vec::new(),
        errors: Vec::new(),
        ..interface()
    }
}

/// The two host files for `interface`, keyed by the paths the emitter
/// promises.
fn emit(interface: &ir::Interface) -> (String, String) {
    let emitter = TsEmitter {
        addon: "sample_ts".to_owned(),
    };
    let files = emitter.emit(interface).expect("emits");
    let read = |path: &str| {
        let file = files
            .iter()
            .position(|file| file.path == path)
            .unwrap_or_else(|| panic!("{path} was not emitted"));
        files[file].contents.clone()
    };
    (read("index.d.ts"), read("index.js"))
}

/// A stream method types as `UnibindStream<T>` and pulls the shared stream
/// declaration in, even though no free function streams.
#[test]
fn a_stream_method_types_and_wraps_like_a_stream_function() {
    let (dts, js) = emit(&namespaced_interface());
    assert!(
        dts.contains("export interface UnibindStream<T> extends AsyncIterable<T>"),
        "a method-only stream did not pull in the stream declaration:\n{dts}"
    );
    assert!(
        dts.contains("  watch(): UnibindStream<string>;"),
        "the stream method is mistyped:\n{dts}"
    );
    assert!(
        js.contains("return wrapStream(this.#handle.watch(...args));"),
        "the stream method hands back the raw native handle:\n{js}"
    );
}

/// A method returning an object hands back the wrapper class, not the bare
/// native handle: the handle decodes no errors and has no disposal, so
/// `index.d.ts` would be declaring something the runtime never produced.
#[test]
fn a_method_returning_an_object_hands_back_the_wrapper_class() {
    let (dts, js) = emit(&namespaced_interface());
    assert!(
        dts.contains("  keys(): Keys;"),
        "the namespacing method is mistyped:\n{dts}"
    );
    assert!(
        js.contains("return new Keys(nativeHandle, this.#handle.keys(...args));"),
        "the namespacing method hands back the raw native handle:\n{js}"
    );
}

/// Records compose: another record under `Option` (optional in both
/// directions) and as a map value.
#[test]
fn records_compose_under_option_and_as_map_values() {
    let (dts, _) = emit(&namespaced_interface());
    assert!(
        dts.contains("  meta?: Meta | null;"),
        "an optional record field is mistyped:\n{dts}"
    );
    assert!(
        dts.contains("  metaByKey: Record<string, Meta>;"),
        "a record-valued map is mistyped:\n{dts}"
    );
}
