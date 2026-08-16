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
use unibind_test_support::fixtures::{arg, docs};
use unibind_test_support::sample::{self, field, function, named, names, owned_string};

fn interface() -> ir::Interface {
    sample::interface("sample_ts", "A sample boundary exercising the ts surface.")
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

/// Every integer width -- wide and narrow -- is `number` in the types and
/// `z.number().int()` in the schemas: the Stripe/OpenAI policy, so records
/// stay plain JSON. One table drives both assertions because one renderer
/// list drives both surfaces -- a change to one that missed the other would
/// leave a schema and a declared type disagreeing about a width, and that
/// is the failure this catches. The checked-range half of the policy (a
/// fractional or unsafe-range `number` is refused, never truncated) lives
/// in the glue and is proven by the ts conformance suite.
#[test]
fn integers_are_numbers_in_the_types_and_the_schemas() {
    let widths = [
        ("total", ir::IntKind::U64, "number", "z.number().int()"),
        ("offset", ir::IntKind::Isize, "number", "z.number().int()"),
        ("size", ir::IntKind::Usize, "number", "z.number().int()"),
        ("count", ir::IntKind::I64, "number", "z.number().int()"),
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

/// Bytes are `Buffer` wherever the glue converts the value itself -- an
/// argument, a return, a stream item, and a record field -- and an array of
/// numbers only inside a container, which crosses whole and unconverted.
///
/// A record field is the position that reads as "nested" but is not: the
/// glue's mirror struct declares it `Buffer`, so declaring `Array<number>`
/// here would be a runtime type error, not a compile error. One table drives
/// the declared type and the schema together, because a schema checking a
/// number array where the type promises a `Buffer` is exactly the drift this
/// catches.
#[test]
fn bytes_are_a_buffer_in_every_position_but_inside_a_container() {
    let bytes = || ir::Type::Bytes { owned: true };
    let positions = [
        ("bare", bytes(), "Buffer", "z.instanceof(Buffer)"),
        (
            "maybe",
            ir::Type::Option(Box::new(bytes())),
            "Buffer | null",
            "z.instanceof(Buffer).nullable().optional()",
        ),
        (
            "listed",
            ir::Type::Vec(Box::new(bytes())),
            "Array<Array<number>>",
            "z.array(z.array(z.number().int()))",
        ),
        (
            "mapped",
            ir::Type::Map {
                key: Box::new(owned_string()),
                value: Box::new(bytes()),
            },
            "Record<string, Array<number>>",
            "z.record(z.string(), z.array(z.number().int()))",
        ),
    ];

    let mut byteful = interface();
    for (name, ty, _, _) in &positions {
        byteful.records[0]
            .fields
            .push(field(name, None, &[], ty.clone()));
    }
    let files = emitter().emit(&byteful).expect("emits");
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
    for (name, _, declared, schema) in positions {
        // `maybe` is an `Option`, which the field declaration spells with a
        // `?` and the schema with `.optional()`; both are in the expected
        // strings above.
        let key = if name == "maybe" {
            format!("{name}?: {declared};")
        } else {
            format!("{name}: {declared};")
        };
        assert!(dts.contains(&key), "`{key}` missing from:\n{dts}");
        assert!(
            schemas.contains(&format!("{name}: {schema},")),
            "`{name}: {schema},` missing from:\n{schemas}"
        );
    }
    // `z.instanceof(Buffer)` reads the constructor at run time, so the
    // schema file needs a value import, not a type-only one.
    assert!(
        schemas.contains("import { Buffer } from \"node:buffer\";"),
        "{schemas}"
    );
}

/// A record holding no bytes gets no `node:buffer` import in its schemas,
/// even when a function signature takes a `Buffer`: `schemas.ts` declares
/// records and nothing else, and an unused import is a type error under
/// `noUnusedLocals`.
#[test]
fn a_buffer_argument_alone_does_not_import_buffer_into_the_schemas() {
    let mut byteless = interface();
    byteless.records[0]
        .fields
        .retain(|existing| !matches!(existing.ty, ir::Type::Bytes { .. }));
    let rendered = schemas(&byteless);
    assert!(!rendered.contains("node:buffer"), "{rendered}");
    // The `touch` export still takes bytes, so the declarations do import it.
    let dts = emitter()
        .emit(&byteless)
        .expect("emits")
        .into_iter()
        .find(|file| file.path == "index.d.ts")
        .expect("index.d.ts is emitted")
        .contents;
    assert!(
        dts.contains("import type { Buffer } from \"node:buffer\";"),
        "{dts}"
    );
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
        associated: Vec::new(),
        methods,
    }
}

/// An object whose methods stream and hand back another object: the shapes
/// the ix SDK's `Machine` and `client.keys().create(...)` need. Kept off
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
        ..function(
            "watch",
            None,
            &["Every event, as a pull stream."],
            Vec::new(),
        )
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
        "return wrapStream(this.#handle.watch(...args.map(normalizeArg)));",
        "return new Keys(nativeHandle, this.#handle.keys(...args.map(normalizeArg)));",
    ] {
        assert!(js.contains(wrapped), "`{wrapped}` is missing:\n{js}");
    }
}

/// Records compose: another record under `Option` (optional in both
/// directions) and as a map value.
#[test]
fn records_compose_under_option_and_as_map_values() {
    let HostFiles { dts, .. } = emit(&namespaced_interface());
    for declared in [
        "  readonly meta?: Meta | null;",
        "  readonly metaByKey: Record<string, Meta>;",
    ] {
        assert!(dts.contains(declared), "`{declared}` is missing:\n{dts}");
    }
}

/// One unit enum, plus the two positions a value of it occupies. Off the
/// shared fixture for the same reason `namespaced_interface` is: these
/// tests state their rule rather than restating a snapshot.
fn enum_interface() -> ir::Interface {
    let severity = ir::Enum {
        name: "Severity".to_owned(),
        names: names(None, None),
        docs: docs(&["How bad it is."]),
        variants: vec![
            ir::EnumVariant {
                name: "Info".to_owned(),
                wire: "info".to_owned(),
                names: names(Some("INFO"), None),
                docs: docs(&["Routine."]),
            },
            ir::EnumVariant {
                name: "HardFailure".to_owned(),
                wire: "hard_failure".to_owned(),
                names: names(Some("HARD_FAILURE"), None),
                docs: Vec::new(),
            },
        ],
    };
    let finding = ir::Record {
        name: "Finding".to_owned(),
        names: names(None, None),
        docs: docs(&["One finding."]),
        fields: vec![field("severity", None, &["How bad it is."], named("Severity"))],
    };
    let worst = ir::Function {
        ret: Some(named("Severity")),
        ..function(
            "worst",
            None,
            &["The worst severity seen."],
            vec![arg("floor", named("Severity"), None)],
        )
    };
    ir::Interface {
        enums: vec![severity],
        records: vec![finding],
        functions: vec![worst],
        errors: Vec::new(),
        objects: Vec::new(),
        ..interface()
    }
}

/// A unit enum is a union of the string literals that cross, never a
/// TypeScript `enum`: the value really is a plain string, so a union is the
/// only declaration `JSON.parse` output satisfies.
#[test]
fn a_unit_enum_declares_a_union_of_string_literals() {
    let HostFiles { dts, .. } = emit(&enum_interface());
    for declared in [
        "export type Severity = \"info\" | \"hard_failure\";",
        // Both positions name the union, not `string`.
        "export declare function worst(floor: Severity): Severity;",
        "  readonly severity: Severity;",
    ] {
        assert!(dts.contains(declared), "`{declared}` is missing:\n{dts}");
    }
    assert!(
        !dts.contains("export enum"),
        "a TypeScript enum is not erasable and its members are not the \
         strings that cross:\n{dts}"
    );
    // A variant's own doc has nowhere of its own to live in a union, so it
    // joins the type's block rather than being dropped.
    assert!(dts.contains(" * - `info`: Routine."), "{dts}");
}

/// The Zod schema comes from the same IR as the declaration, so a consumer
/// validating at run time checks exactly the set the type promises.
#[test]
fn a_unit_enum_gets_a_zod_schema_and_infers_back_to_the_union() {
    let emitter = TsEmitter {
        addon: "sample_ts".to_owned(),
    };
    let files = emitter.emit(&enum_interface()).expect("emits");
    let schemas = files
        .iter()
        .find(|file| file.path == "schemas.ts")
        .expect("schemas.ts is emitted")
        .contents
        .clone();
    for declared in [
        "export const Severity = z.enum([\"info\", \"hard_failure\"]).describe(\"How bad it is.\");",
        "export type Severity = z.infer<typeof Severity>;",
        // The record reads the enum schema by name, with no `z.lazy` thunk:
        // enumerations are all bound before the first record.
        "    severity: Severity.describe(\"How bad it is.\"),",
    ] {
        assert!(
            schemas.contains(declared),
            "`{declared}` is missing:\n{schemas}"
        );
    }
}
