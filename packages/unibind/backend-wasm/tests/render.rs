//! Snapshot the lowering and the `wasm-bindgen` render for the sample module.
//! The committed snapshots are the review surface for what the macro generates;
//! on drift the test prints the new content to copy over the snapshot file.
//! (trybuild/macrotest would invoke cargo at test runtime, which the nix test
//! sandbox cannot do, so the render output is snapshotted directly.)
//!
//! Rules that hold across interfaces -- how a class is named, what an async
//! export hands back, what a record crosses as -- are asserted on their own
//! small lowered modules instead, so the rule is stated where it is checked
//! rather than left for a reader to infer from a 600-line snapshot.

use unibind_core::ir;
use unibind_test_support::{assert_ir_json_snapshot, assert_render_snapshot, lower_module_source};

const GLUE_SNAPSHOT: &str = include_str!("snapshots/sample.wasm.rs");

fn interface() -> ir::Interface {
    lower_module_source(include_str!("fixtures/sample.rs"))
}

#[test]
fn ir_json_snapshot() {
    assert_ir_json_snapshot(
        &interface(),
        include_str!("snapshots/sample.ir.json"),
        "sample.ir.json",
    );
}

#[test]
fn wasm_glue_snapshot() {
    let interface = interface();
    let rendered = unibind_backend_wasm::render(&interface).expect("renders");

    assert_render_snapshot!(interface, rendered, GLUE_SNAPSHOT, "sample.wasm.rs");
}

/// The rendered glue as readable Rust, for tests that assert on one item rather
/// than on the whole committed snapshot.
fn glue_source(source: &str) -> String {
    let interface = lower_module_source(source);
    let rendered = unibind_backend_wasm::render(&interface).expect("renders");
    let file: syn::File = syn::parse2(rendered.glue).expect("glue parses");
    prettyplease::unparse(&file)
}

/// The message a refused interface carries.
fn render_failure(source: &str) -> String {
    let interface = lower_module_source(source);
    let ::std::result::Result::Err(error) = unibind_backend_wasm::render(&interface) else {
        panic!("the wasm render accepts unsupported surface");
    };
    error.message
}

/// `source` with every whitespace character removed and every trailing comma
/// dropped, so an assertion about a declaration cannot fail on where
/// prettyplease decided to wrap it -- wrapping a parameter list is exactly what
/// adds the trailing comma, so the two normalizations are one rule.
fn packed(source: &str) -> String {
    let dense: String = source.chars().filter(|c| !c.is_whitespace()).collect();
    let mut packed = dense;
    for closer in [')', '}', '>', ']'] {
        packed = packed.replace(&format!(",{closer}"), &closer.to_string());
    }
    // A needle quoted down to one field or one match arm ends at the comma the
    // glue's own normalization just dropped.
    packed.trim_end_matches(',').to_owned()
}

fn assert_contains(glue: &str, needles: &[&str]) {
    let packed_glue = packed(glue);
    let missing: Vec<&&str> = needles
        .iter()
        .filter(|needle| !packed_glue.contains(&packed(needle)))
        .collect();
    assert!(
        missing.is_empty(),
        "missing from the glue: {missing:?}\n{glue}"
    );
}

/// Every one of `forbidden` is absent. Collected rather than asserted one at a
/// time so that a render which grew several of them at once says so, and so
/// that each entry is independently exercised when one is planted.
fn assert_absent(glue: &str, forbidden: &[&str]) {
    let packed_glue = packed(glue);
    let present: Vec<&&str> = forbidden
        .iter()
        .filter(|needle| packed_glue.contains(&packed(needle)))
        .collect();
    assert!(
        present.is_empty(),
        "present in the glue and must not be: {present:?}\n{glue}"
    );
}

/// Nothing exported renders as an `async fn`: `wasm-bindgen`'s support for one
/// with a `&self` receiver inside an exported impl is version-dependent, so an
/// async export is a sync fn handing back the `Promise` the caller would have
/// received anyway.
#[test]
fn async_exports_hand_back_promises() {
    let glue = glue_source(include_str!("fixtures/sample.rs"));
    assert_absent(&glue, &["pub async fn"]);
    assert_contains(
        &glue,
        &[
            // A free function, a method, and the generated resource close.
            "pub fn slow_add(a: f64, b: f64, __unibind_signal: \
             ::std::option::Option<::js_sys::Object>) -> ::js_sys::Promise",
            "pub fn add(&self, amount: f64, __unibind_signal: \
             ::std::option::Option<::js_sys::Object>) -> ::js_sys::Promise",
            "pub fn close(&self) -> ::js_sys::Promise",
            "::wasm_bindgen_futures::future_to_promise(async move {",
        ],
    );
    // A sync export takes no signal: there is nothing to abort.
    assert_contains(&glue, &["pub fn checksum(data: ::std::vec::Vec<u8>) -> u32"]);
}

/// The abort bridge renders only where something async can use it.
#[test]
fn the_abort_bridge_renders_only_for_async_exports() {
    let sync_only = glue_source("mod m { pub fn go() -> u32 { 0 } }");
    assert_absent(&sync_only, &["__unibind_wasm_with_abort", "__UnibindWasmAbortSignal"]);
    let with_async = glue_source("mod m { pub async fn go() -> u32 { 0 } }");
    assert_contains(
        &with_async,
        &[
            "pub type __UnibindWasmAbortSignal;",
            "fn aborted(this: &__UnibindWasmAbortSignal) -> bool;",
            // Registered and unregistered around the race: one signal outlives
            // one call, and a dropped closure invoked from JavaScript throws.
            "signal.add_event_listener(\"abort\", callback);",
            "signal.remove_event_listener(\"abort\", callback);",
            "__unibind_wasm_error(::std::string::String::from(\"__unibind__:aborted\"))",
        ],
    );
}

/// Every stream-returning export gets its own handle class, named for its
/// owner. A free `tail` and a `Store::tail` are both streams, and one class
/// serving both would silently misbind them.
#[test]
fn stream_classes_are_owner_scoped() {
    let glue = glue_source(
        "mod m {
            #[unibind::object]
            pub struct Store {}

            impl Store {
                pub fn tail(&self) -> unibind_runtime::UniStream<i64> {
                    todo!()
                }
            }

            pub fn tail() -> unibind_runtime::UniStream<i64> {
                todo!()
            }
        }",
    );
    for class in ["__UnibindWasmStreamTail", "__UnibindWasmStreamStoreTail"] {
        // The declaration, not the bare name: one class name can be a prefix of
        // another, and a bare `contains` would pass on the wrong one.
        let declaration = format!("pub struct {class} {{");
        assert!(
            glue.contains(&declaration),
            "`{class}` is missing from the glue:\n{glue}"
        );
    }
    assert_contains(
        &glue,
        &[
            "__UnibindWasmStreamStoreTail::__unibind_from",
            // The pull state sits behind an `Arc` because `next()` hands back a
            // `Promise`, whose future cannot borrow `&self`.
            "stream: ::std::sync::Arc<::unibind_runtime::PullStream<i64>>,",
            "let __unibind_stream = ::std::sync::Arc::clone(&self.stream);",
            "::std::option::Option::None =>",
            "::std::result::Result::Ok(::wasm_bindgen::JsValue::NULL)",
        ],
    );
}

/// A method returning another object crosses as that object's handle class,
/// which is what makes `client.keys().create(...)` chain.
#[test]
fn a_method_returning_an_object_crosses_as_its_handle() {
    let glue = glue_source(
        "mod m {
            #[unibind::object]
            pub struct Keys {}

            impl Keys {
                pub fn create(&self) -> String {
                    todo!()
                }
            }

            #[unibind::object]
            pub struct Client {}

            impl Client {
                pub fn keys(&self) -> Keys {
                    todo!()
                }
            }
        }",
    );
    assert_contains(&glue, &["__UnibindWasmObjectKeys::__unibind_from"]);
}

/// An associated function is a static on the class, and one returning the
/// object needs no marker: `wasm-bindgen` makes any receiver-less function in an
/// exported impl a static, and a static handing back the class is the factory.
#[test]
fn an_associated_function_renders_as_a_static() {
    let glue = glue_source(
        "mod m {
            #[unibind::object]
            pub struct Machine {}

            impl Machine {
                #[unibind(associated)]
                pub async fn oci(image: String) -> Machine {
                    todo!()
                }
            }
        }",
    );
    assert_contains(
        &glue,
        &[
            "#[wasm_bindgen(js_name = \"oci\")]",
            "pub fn oci(image: ::std::string::String, __unibind_signal: \
             ::std::option::Option<::js_sys::Object>) -> ::js_sys::Promise",
            "::wasm_bindgen::JsValue::from(__UnibindWasmObjectMachine::__unibind_from(value))",
        ],
    );
    // wasm-bindgen has no factory marker to emit.
    assert_absent(&glue, &["factory"]);
}

/// The wasm backend names its unsupported surface instead of miscompiling.
#[test]
fn unsupported_surface_is_named() {
    for (source, needle) in [
        (
            "mod m { use std::collections::HashMap; pub fn go(map: HashMap<u32, bool>) {} }",
            "non-string keys",
        ),
        (
            "mod m { use std::collections::HashMap; \
             #[unibind::record] pub struct R { pub m: HashMap<u32, bool> } }",
            "non-string keys",
        ),
        // `blocking` frees Python's GIL. There is no thread here to free, so the
        // flag is a promise the boundary cannot keep.
        (
            "mod m { #[unibind(blocking)] pub fn checksum(data: &[u8]) -> u32 { 0 } }",
            "`checksum` is `blocking`",
        ),
        (
            "mod m {
                #[unibind::object]
                pub struct Counter {}

                impl Counter {
                    #[unibind(blocking)]
                    pub fn tick(&self) -> u32 { 0 }
                }
            }",
            "`Counter.tick` is `blocking`",
        ),
    ] {
        let message = render_failure(source);
        assert!(message.contains(needle), "{message}");
    }
    // The refusal names the idiom owed, not just the refusal.
    let message = render_failure("mod m { #[unibind(blocking)] pub fn go() {} }");
    assert!(message.contains("`async fn`"), "{message}");
    assert!(message.contains("`Promise`"), "{message}");
}

/// Every 64-bit width renders, in both directions, as a checked `f64` -- a
/// JavaScript `number` -- never as a `bigint` and never as a plain Rust integer
/// the boundary would truncate silently: the inbound narrowing helper is what
/// makes the `f64` declaration safe. One `.d.ts` vocabulary is the point; half
/// of it typed `bigint` would be a second one.
#[test]
fn wide_integers_render_as_checked_numbers() {
    for source in [
        "mod m { pub fn go(count: u64) {} }",
        "mod m { pub fn go() -> usize { 0 } }",
        "mod m { pub fn go(offset: isize) -> i64 { 0 } }",
        "mod m { pub fn go(counts: Vec<u64>) -> Option<i64> { None } }",
        "mod m { #[unibind::record] pub struct R { pub size: usize } }",
    ] {
        let interface = lower_module_source(source);
        let rendered = unibind_backend_wasm::render(&interface).expect("renders");
        let glue = rendered.glue.to_string();
        assert_absent(&glue, &["BigInt", "bigint", ": u64", ": usize", ": isize"]);
        assert!(glue.contains("f64"), "no f64 in the glue for `{source}`:\n{glue}");
    }
    let glue = glue_source("mod m { pub fn go(count: u64) -> u64 { count } }");
    assert_contains(
        &glue,
        &[
            "let count = __unibind_wasm_number_to_u64(count).map_err(__unibind_wasm_error)?;",
            "value as f64",
            "const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;",
        ],
    );
}

/// A record crosses through its generated serde twin, and the user's own struct
/// gains nothing: `wasm-bindgen` has no attribute that would make the struct
/// itself cross, so there is no second claim on the same JavaScript shape.
/// Nested records recurse into nested twins, so one serde move carries the tree.
#[test]
fn records_cross_through_a_serde_twin() {
    const SOURCE: &str = "mod m {
            #[unibind::record]
            #[derive(Clone)]
            pub struct Inner {
                pub size: u64,
            }

            #[unibind::record]
            #[derive(Clone)]
            pub struct Outer {
                pub the_inner: Inner,
                pub items: Vec<Inner>,
                pub home: Option<std::path::PathBuf>,
            }

            pub fn echo(outer: Outer) -> Outer { outer }
        }";
    let glue = glue_source(SOURCE);
    assert_contains(
        &glue,
        &[
            "#[derive(::serde::Serialize, ::serde::Deserialize)] pub struct __UnibindWasmRecordInner {",
            "#[serde(rename = \"size\")] pub size: f64,",
            // A field key is the JavaScript name, and an optional field is one a
            // caller omits rather than sets to `undefined`.
            "#[serde(rename = \"theInner\")] pub the_inner: __UnibindWasmRecordInner,",
            "#[serde(rename = \"home\")] #[serde(default)] pub home: \
             ::std::option::Option<::std::path::PathBuf>,",
            "pub items: ::std::vec::Vec<__UnibindWasmRecordInner>,",
            // Both directions recurse, and only the inbound one can refuse.
            "the_inner: __UnibindWasmRecordInner::__unibind_from(value.the_inner),",
            "the_inner: __UnibindWasmRecordInner::__unibind_into(self.the_inner)?,",
            // The whole record crosses as one `JsValue`.
            "pub fn echo(outer: ::wasm_bindgen::JsValue)",
            "__unibind_wasm_from_js::<__UnibindWasmRecordOuter>(outer)",
            "__unibind_wasm_to_js::<__UnibindWasmRecordOuter>(&__UnibindWasmRecordOuter::__unibind_from(value))",
        ],
    );
    let rendered = unibind_backend_wasm::render(&lower_module_source(SOURCE)).expect("renders");
    let decorated: Vec<usize> = rendered
        .records
        .iter()
        .enumerate()
        .filter(|(_, attrs)| !attrs.outer.is_empty() || !attrs.fields.iter().all(Vec::is_empty))
        .map(|(index, _)| index)
        .collect();
    assert!(
        decorated.is_empty(),
        "records {decorated:?} gained attributes the user never asked for"
    );
    assert_eq!(rendered.records.len(), 2, "one entry per IR record");
    assert_eq!(
        rendered.records[1].fields.len(),
        3,
        "field attributes stay index-aligned with the IR"
    );
}

/// A unit enum crosses as its wire string, with both halves of the mapping
/// generated. Inbound refuses a word outside the set and lists the set.
#[test]
fn unit_enums_cross_as_their_wire_string() {
    let glue = glue_source(
        "mod m {
            #[unibind::enumeration]
            pub enum Severity {
                Info,
                NotFound,
            }

            #[unibind::record]
            #[derive(Clone)]
            pub struct R {
                pub level: Severity,
            }

            pub fn go(level: Severity) -> Severity { level }
        }",
    );
    assert_contains(
        &glue,
        &[
            "fn __unibind_wasm_enum_to_str_Severity(value: __unibind_user::Severity)",
            "__unibind_user::Severity::NotFound => \"not_found\",",
            "is not a Severity; expected one of info, not_found",
            // The argument, the return, and a record field are all the string.
            "pub fn go(level: ::std::string::String)",
            "let level = __unibind_wasm_enum_from_str_Severity(level).map_err(__unibind_wasm_error)?;",
            "pub level: ::std::string::String,",
        ],
    );
}

/// Bytes cross as a `Uint8Array` where `wasm-bindgen` carries them (a whole
/// argument, return, or stream item) and as serde's array of numbers inside a
/// record or a container. The split is the position, not the depth.
#[test]
fn bytes_cross_as_uint8array_only_where_wasm_bindgen_carries_them() {
    let glue = glue_source(
        "mod m {
            #[unibind::record]
            #[derive(Clone)]
            pub struct R {
                pub blob: Vec<u8>,
            }

            pub fn echo(data: &[u8]) -> Vec<u8> { data.to_vec() }
            pub fn wrap(r: R) -> R { r }
        }",
    );
    assert_contains(
        &glue,
        &[
            "pub fn echo(data: ::std::vec::Vec<u8>) -> ::std::vec::Vec<u8>",
            "__unibind_user::echo(data.as_slice())",
            // Inside the twin it is serde's, which spells a byte string as an
            // array of numbers; nothing converts it either way.
            "pub blob: ::std::vec::Vec<u8>,",
            "blob: value.blob,",
            "blob: self.blob,",
        ],
    );
    let promised = glue_source("mod m { pub async fn go() -> Vec<u8> { Vec::new() } }");
    assert_contains(
        &promised,
        &["::js_sys::Uint8Array::from(&value[..])"],
    );
}

/// A path crosses as a JavaScript string: `wasm-bindgen` has no `PathBuf`, and
/// a path that is not valid UTF-8 is refused rather than mangled -- the same
/// verdict serde reaches for a path inside a record.
#[test]
fn paths_cross_as_strings() {
    let glue = glue_source(
        "mod m {
            pub fn go(path: &std::path::Path) -> std::path::PathBuf { path.to_path_buf() }
        }",
    );
    assert_contains(
        &glue,
        &[
            "pub fn go(path: ::std::string::String)",
            "__unibind_user::go(::std::path::Path::new(path.as_str()))",
            "__unibind_wasm_path_to_string(value).map_err(__unibind_wasm_error)",
        ],
    );
}

/// The generated `close()` is idempotent, and no `Drop` warns about a leak: a
/// wasm handle's `free()` is the JavaScript side's call to make, and neither
/// engine promises it ever runs, so a warning would fire on some exits and not
/// others. Detecting the leak belongs to a `FinalizationRegistry` in the
/// wrapper.
#[test]
fn a_resource_closes_once_and_warns_never() {
    let glue = glue_source(include_str!("fixtures/sample.rs"));
    assert_contains(
        &glue,
        &[
            "closed: ::std::sync::atomic::AtomicBool,",
            "let __unibind_first = !self.closed.swap(true, ::std::sync::atomic::Ordering::SeqCst);",
        ],
    );
    // `Drop` appears in prose ("drop the stream early"), so the assertion is
    // about the impl: nothing generated implements it.
    assert_absent(&glue, &["impl ::std::ops::Drop", "fn drop("]);
}

/// The glue reaches the user's module through exactly one alias, bound at the
/// glue module's own scope, and never writes `super::` anywhere else. A binding
/// library that relocates the items it expands (napi-derive does) turns any
/// other `super::` into a hop that lands one level short of the crate root, and
/// an adopter cannot work around it.
#[test]
fn the_glue_reaches_the_user_module_only_through_its_alias() {
    let glue = glue_source(include_str!("fixtures/sample.rs"));
    let hops: Vec<&str> = glue
        .lines()
        .map(str::trim)
        .filter(|line| line.contains("super::"))
        .collect();
    assert_eq!(
        hops,
        vec!["use super::sample_wasm as __unibind_user;"],
        "the glue must spell `super::` only in its own alias binding"
    );
    assert!(
        glue.contains("__unibind_user::Row"),
        "named types must resolve through the alias:\n{glue}"
    );
}
