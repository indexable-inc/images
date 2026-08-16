//! Snapshot the browser host files for a representative interface. The
//! committed snapshots are the review surface for what `unibind-gen wasm`
//! writes; on drift the test prints the new content to copy over the snapshot
//! file. The interface is built literally (every IR field is `pub`) so the
//! fixture covers renames, docs, defaults, the async cancellation surface,
//! streams, and objects without lowering Rust source.
//!
//! It is deliberately the same fixture `ts.rs` renders: the two targets
//! publish one surface, so the two snapshot sets diff against each other and
//! every difference between them is a policy this file states a rule for.

use unibind_core::ir;
use unibind_gen::host::HostEmitter as _;
use unibind_gen::wasm::WasmEmitter;
use unibind_test_support::assert_host_snapshots;
use unibind_test_support::fixtures::docs;
use unibind_test_support::sample::{self, field, function, named, names, owned_string};

fn interface() -> ir::Interface {
    sample::interface("sample_wasm", "A sample boundary exercising the wasm surface.")
}

/// The module specifier the fixture's wrapper imports; a relative specifier
/// with a `.js` extension, which is what `wasm-bindgen --target web` writes
/// and what a browser can resolve without a bundler.
const MODULE: &str = "./wasm/sample.js";

fn emitter() -> WasmEmitter {
    WasmEmitter {
        module: MODULE.to_owned(),
    }
}

/// The three host files the wasm emitter writes for one interface.
struct HostFiles {
    dts: String,
    schemas: String,
    js: String,
}

/// Emit `interface` and pick the files by name, so a change to the emit order
/// does not disturb the assertions.
fn emit(interface: &ir::Interface) -> HostFiles {
    let files = emitter().emit(interface).expect("emits");
    let read = |path: &str| {
        let index = files
            .iter()
            .position(|file| file.path == path)
            .unwrap_or_else(|| panic!("{path} was not emitted"));
        files[index].contents.clone()
    };
    HostFiles {
        dts: read("index.d.ts"),
        schemas: read("schemas.ts"),
        js: read("index.js"),
    }
}

#[test]
fn wasm_host_files_snapshot() {
    let files = emitter().emit(&interface()).expect("emits");
    assert_host_snapshots(
        files
            .iter()
            .map(|file| (file.path.as_str(), file.contents.as_str())),
        &[
            (
                "index.d.ts",
                "sample.browser.d.ts",
                include_str!("snapshots/sample.browser.d.ts"),
            ),
            (
                "schemas.ts",
                "sample.browser.schemas.ts",
                include_str!("snapshots/sample.browser.schemas.ts"),
            ),
            (
                "index.js",
                "sample.browser.js",
                include_str!("snapshots/sample.browser.js"),
            ),
        ],
    );
}

/// The wrapper is an ES module, not `CommonJS`: a browser loads it with a
/// `<script type="module">` and there is no `require` to reach for. The
/// `wasm-bindgen --target web` output it wraps is itself an ES module, so
/// nothing else was available.
#[test]
fn the_wrapper_is_an_es_module() {
    let HostFiles { js, .. } = emit(&interface());
    for absent in ["require(", "module.exports", "\"use strict\""] {
        assert!(!js.contains(absent), "`{absent}` is in:\n{js}");
    }
    assert!(
        js.contains("import __wasm_init, {\n"),
        "the module head is missing:\n{js}"
    );
    assert!(js.contains("export {\n  rows,\n"), "{js}");
}

/// The initializer is the wasm module's own default export, and nothing the
/// wrapper defines runs before it is awaited, so both generated files hand it
/// straight through -- typed by the module's own declarations rather than
/// re-declared here.
#[test]
fn the_initializer_is_re_exported_under_both_names() {
    let HostFiles { dts, js, .. } = emit(&interface());
    assert!(
        js.contains("export { __wasm_init as default, __wasm_init as init };"),
        "{js}"
    );
    assert!(
        dts.contains(&format!(
            "export {{ default, default as init }} from \"{MODULE}\";"
        )),
        "{dts}"
    );
}

/// Every compiled export the wrapper calls is imported under an alias,
/// because the wrapper defines a function or a class under the export's own
/// name. A class the wrapper never names is not imported at all.
#[test]
fn the_compiled_exports_are_imported_under_aliases() {
    let HostFiles { js, .. } = emit(&interface());
    for aliased in [
        "  Counter as __wasm_Counter,",
        "  rows as __wasm_rows,",
        "  touchPath as __wasm_touchPath,",
        &format!("}} from \"{MODULE}\";"),
    ] {
        assert!(js.contains(aliased), "`{aliased}` is missing:\n{js}");
    }
    // The class the wrapper constructs is reached through its alias; the
    // wrapper's own class keeps the published name.
    assert!(
        js.contains("this.#handle = new __wasm_Counter(...args);"),
        "{js}"
    );
    assert!(js.contains("class Counter {"), "{js}");

    // An object nobody constructs and nothing calls a static on arrives only
    // as a return value, so importing its class would bind a name no line
    // reads.
    let HostFiles { js, .. } = emit(&namespaced_interface());
    for absent in ["Keys as __wasm_Keys", "Client as __wasm_Client"] {
        assert!(!js.contains(absent), "`{absent}` is in:\n{js}");
    }
}

/// Bytes are a `Uint8Array` exactly where the `wasm-bindgen` signature
/// carries them itself -- a whole argument, return, or stream item -- and an
/// array of numbers everywhere serde carries them, which is a record field as
/// much as a container's interior. That second half is where this differs
/// from node: napi's mirror struct declares a `Buffer` field, serde's twin
/// does not, and declaring one here would be a runtime type error rather than
/// a compile error.
#[test]
fn bytes_are_a_uint8array_only_where_the_signature_carries_them() {
    let bytes = || ir::Type::Bytes { owned: true };
    let positions = [
        ("bare", bytes(), "Array<number>", "z.array(z.number().int())"),
        (
            "maybe",
            ir::Type::Option(Box::new(bytes())),
            "Array<number> | null",
            "z.array(z.number().int()).nullable().optional()",
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
    let HostFiles { dts, schemas, .. } = emit(&byteful);
    for (name, _, declared, schema) in positions {
        // `maybe` is an `Option`, which the field declaration spells with a
        // `?` and the schema with `.optional()`.
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
    // The signature positions, where the glue builds a `Uint8Array` view
    // instead (`unibind_backend_wasm::ty::js_value`).
    assert!(
        dts.contains("touchPath(path: string, data: Uint8Array, ratio?: number): boolean;"),
        "{dts}"
    );
}

/// A browser has no `Buffer` at all, so no generated file may name one: not
/// the declarations, not the schemas' runtime check, and not an import of
/// `node:buffer`.
#[test]
fn no_generated_file_names_a_buffer() {
    let HostFiles { dts, schemas, js } = emit(&interface());
    for (path, contents) in [("index.d.ts", &dts), ("schemas.ts", &schemas), ("index.js", &js)] {
        assert!(
            !contents.contains("Buffer"),
            "{path} names a Buffer:\n{contents}"
        );
    }
}

/// The stream handle resolves the element or `null`, so `null` is what ends
/// the iteration. Both glues promise it (`backend-wasm/src/stream.rs` returns
/// `JsValue::NULL` past the end), which is why one helper serves both.
#[test]
fn wrapped_streams_end_on_null() {
    let HostFiles { dts, js, .. } = emit(&interface());
    assert!(
        js.contains("const item = await stream.next();\n          if (item === null) {"),
        "{js}"
    );
    assert!(dts.contains("next(): Promise<T | null>;"), "{dts}");
}

/// The leak warning the Rust glue leaves to JavaScript: an open resource
/// wrapper that gets collected says so once. Every path that binds a handle
/// registers -- including a wrapper built around a returned handle -- and
/// closing unregisters, so a closed resource is silent. The guard is what
/// keeps a runtime without a `FinalizationRegistry` merely quiet instead of
/// broken.
#[test]
fn an_open_resource_is_watched_for_a_missing_close() {
    let HostFiles { js, .. } = emit(&interface());
    for present in [
        "typeof FinalizationRegistry === \"function\"",
        "console.warn(`unclosed ${name}: call close() or use \\`await using\\``);",
        "      this.#handle = args[1];\n      watchForLeak(this, \"Counter\");",
        "    }\n    watchForLeak(this, \"Counter\");\n  }",
        "  async close(...args) {\n    leakWatchClosed(this);",
        "  async [Symbol.asyncDispose]() {\n    await this.close();",
    ] {
        assert!(js.contains(present), "`{present}` is missing:\n{js}");
    }
}

/// An interface with no resource has nothing to watch, so neither the
/// registry nor a registration renders. A plain object is not a resource: it
/// has no close to forget.
#[test]
fn an_interface_without_a_resource_has_no_registry() {
    let HostFiles { js, .. } = emit(&namespaced_interface());
    for absent in ["FinalizationRegistry", "watchForLeak", "leakWatchClosed"] {
        assert!(!js.contains(absent), "`{absent}` is in:\n{js}");
    }
    // The objects are there; it is the watching that is not.
    assert!(js.contains("class Keys {"), "{js}");
}

/// Arguments are forwarded exactly as the caller passed them. node's wrapper
/// rewrites `null` to `undefined` because napi reads absence only from
/// `undefined`; the wasm boundary reads both as absent already
/// (`serde_wasm_bindgen`'s `is_nullish` is `loose_eq(null)`, and
/// `wasm-bindgen`'s glue tests `x === undefined || x === null`), so a
/// normalization pass here would be a rewrite with nothing to fix.
#[test]
fn arguments_are_forwarded_unnormalized() {
    let HostFiles { js, .. } = emit(&interface());
    assert!(!js.contains("normalizeArg"), "{js}");
    for forwarded in [
        "    return __wasm_rows(...args);",
        "    return await __wasm_slowAdd(...args);",
        "      return await this.#handle.addSlowly(...args);",
    ] {
        assert!(js.contains(forwarded), "`{forwarded}` is missing:\n{js}");
    }
}

/// The wasm glue raises the same `__unibind__:` messages the napi glue does
/// (`backend-wasm/src/error.rs` says so on purpose), so the decoder is the
/// same one: the reason splits into the generated classes, and an abort
/// becomes the platform's `AbortError`.
#[test]
fn the_decoder_reads_the_glues_own_channel() {
    let HostFiles { js, .. } = emit(&interface());
    for present in [
        "    StoreGone: (message) => new StoreGoneError(message),",
        "  const rest = reason.slice(\"__unibind__:\".length);",
        "  if (rest === \"aborted\") {",
        "    return new DOMException(\"This operation was aborted\", \"AbortError\");",
        // Anchored at column 0 on purpose: the shared decoder's tail is
        // written flush left and node's committed snapshot has it that way
        // too, so matching the indented form would assert a difference
        // between the two targets that does not exist.
        "\nif (rest.startsWith(\"err:\")) {",
    ] {
        assert!(js.contains(present), "`{present}` is missing:\n{js}");
    }
    // The one host-specific word in the shared decoder.
    assert!(
        js.contains("// The web platform's convention for cancelled operations"),
        "{js}"
    );
}

/// Every integer width is a `number` in the types and `z.number().int()` in
/// the schemas, the same policy node ships: the wasm glue declares a 64-bit
/// value `f64` and checks the range on the way in, so records stay plain JSON
/// on both targets rather than half of the vocabulary being `bigint`.
#[test]
fn integers_are_numbers_in_the_types_and_the_schemas() {
    let widths = [
        ("total", ir::IntKind::U64),
        ("offset", ir::IntKind::Isize),
        ("size", ir::IntKind::Usize),
        ("count", ir::IntKind::I64),
        ("narrow", ir::IntKind::U32),
    ];
    let mut wide = interface();
    for (name, kind) in widths {
        wide.functions.push(ir::Function {
            ret: Some(ir::Type::Int(kind)),
            ..function(name, None, &[], Vec::new())
        });
        wide.records[0]
            .fields
            .push(field(name, None, &[], ir::Type::Int(kind)));
    }
    let HostFiles { dts, schemas, .. } = emit(&wide);
    for (name, _) in widths {
        assert!(
            dts.contains(&format!("export declare function {name}(): number;")),
            "{dts}"
        );
        assert!(
            schemas.contains(&format!("{name}: z.number().int(),")),
            "{schemas}"
        );
    }
    assert!(!dts.contains("bigint"), "{dts}");
}

/// What an object's methods hand back, ported from the node assertions: a
/// stream types as the shared `UnibindStream<T>` and arrives as the
/// `AsyncIterable`, and an object return arrives as the wrapper class, since a
/// bare handle decodes no errors and has no disposal.
#[test]
fn object_methods_wrap_their_stream_and_object_returns() {
    let HostFiles { dts, js, .. } = emit(&namespaced_interface());
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

/// A refusal names the backend that owes the idiom, not the sibling target's:
/// the wasm glue is what would have had to carry the map, and
/// `serde_wasm_bindgen` carries a JavaScript object, whose keys are strings.
#[test]
fn an_integer_keyed_map_is_refused_naming_the_wasm_backend() {
    let mut keyed = interface();
    keyed.records[0].fields.push(field(
        "by_index",
        None,
        &[],
        ir::Type::Map {
            key: Box::new(ir::Type::Int(ir::IntKind::U32)),
            value: Box::new(owned_string()),
        },
    ));
    let error = emitter()
        .emit(&keyed)
        .expect_err("an integer-keyed map has no declaration");
    assert!(
        error.message.contains("not part of the wasm backend yet"),
        "{}",
        error.message
    );
}

/// A record-less interface would get a `schemas.ts` holding nothing but an
/// unused `zod` import, so it gets no file at all -- the same rule node
/// follows, from the same shared emit.
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

/// An interface whose objects stream and hand back other objects, and whose
/// objects are all plain: the shapes the ix SDK's namespaces need, and the
/// control for every "no resource, no registry" assertion above.
fn namespaced_interface() -> ir::Interface {
    let meta = ir::Record {
        name: "Meta".to_owned(),
        names: names(None, None),
        docs: docs(&["Row provenance."]),
        fields: vec![field("source", None, &[], owned_string())],
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
        records: vec![meta],
        functions: Vec::new(),
        errors: Vec::new(),
        ..interface()
    }
}

