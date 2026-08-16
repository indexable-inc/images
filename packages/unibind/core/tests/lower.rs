//! Lowering across the phase 0 surface: the golden IR for a full-featured
//! module, and the positioned errors for everything out of scope.

use proc_macro2::TokenStream;
use unibind_core::ir;

fn lower(source: &str) -> Result<ir::Interface, unibind_core::LowerError> {
    lower_with(TokenStream::new(), source)
}

fn lower_with(args: TokenStream, source: &str) -> Result<ir::Interface, unibind_core::LowerError> {
    let file: syn::File = syn::parse_str(source).expect("fixture parses");
    let Some(syn::Item::Mod(module)) = file.items.first() else {
        panic!("fixture starts with a module");
    };
    unibind_core::lower_module(args, module)
}

fn error_message(source: &str) -> String {
    lower(source).expect_err("lowering should fail").message
}

const FULL: &str = r#"
/// A sample boundary.
mod sample {
    use std::collections::HashMap;
    use std::path::PathBuf;

    /// A row.
    #[unibind::record]
    #[derive(Clone)]
    pub struct Row {
        /// Identifier.
        pub id: u64,
        #[unibind(py(name = "label"))]
        pub name: String,
        pub tags: Vec<String>,
        pub weights: HashMap<String, f64>,
        pub blob: Vec<u8>,
        pub home: Option<PathBuf>,
    }

    /// Boundary failures.
    #[unibind::error(py(base = "RuntimeError"))]
    pub enum SampleError {
        /// The store is gone.
        #[unibind(py(name = "StoreGoneError"))]
        StoreGone { message: String },
        /// Bad input.
        Invalid(String),
    }

    /// Count rows.
    pub fn rows(
        store: &str,
        #[unibind(default = 10)] limit: usize,
        root: Option<&str>,
    ) -> Result<Vec<Row>, SampleError> {
        let _ = (store, limit, root);
        Ok(Vec::new())
    }

    #[unibind(py(name = "touch_path"))]
    pub fn touch(path: &std::path::Path, data: &[u8], #[unibind(default = 0.5)] ratio: f64) -> bool {
        let _ = (path, data, ratio);
        true
    }

    fn helper() {}
}
"#;

#[test]
fn lowers_the_full_surface() {
    let interface = lower(FULL).expect("lowering succeeds");
    assert_eq!(interface.version, ir::IR_VERSION);
    assert_eq!(interface.name, "sample");
    assert_eq!(interface.docs, vec!["A sample boundary.".to_owned()]);

    let [rows, touch] = interface.functions.as_slice() else {
        panic!("two exported functions (the private helper is skipped)");
    };
    assert_eq!(rows.name, "rows");
    assert!(matches!(rows.ret, Some(ir::Type::Vec(_))));
    assert_eq!(rows.throws.as_deref(), Some("SampleError"));
    assert!(matches!(rows.args[0].ty, ir::Type::String { owned: false }));
    assert!(matches!(rows.args[1].default, Some(ir::Literal::Int(10))));
    assert!(matches!(rows.args[2].ty, ir::Type::Option(_)));

    assert_eq!(touch.names.py.as_deref(), Some("touch_path"));
    assert!(matches!(touch.args[0].ty, ir::Type::Path { owned: false }));
    assert!(matches!(touch.args[1].ty, ir::Type::Bytes { owned: false }));
    assert!(touch.throws.is_none());
    assert!(matches!(touch.ret, Some(ir::Type::Bool)));

    let [row] = interface.records.as_slice() else {
        panic!("one record");
    };
    assert_eq!(row.fields[1].names.py.as_deref(), Some("label"));
    assert!(matches!(row.fields[4].ty, ir::Type::Bytes { owned: true }));
    assert!(matches!(row.fields[5].ty, ir::Type::Option(_)));

    let [error] = interface.errors.as_slice() else {
        panic!("one error enum");
    };
    assert_eq!(error.py_base.as_deref(), Some("RuntimeError"));
    assert_eq!(
        error.variants[0].names.py.as_deref(),
        Some("StoreGoneError")
    );
    assert_eq!(error.variants[1].name, "Invalid");
}

#[test]
fn ir_round_trips_through_json() {
    let interface = lower(FULL).expect("lowering succeeds");
    let json = serde_json::to_string(&interface).expect("serializes");
    let back: ir::Interface = serde_json::from_str(&json).expect("deserializes");
    assert_eq!(back.functions.len(), interface.functions.len());
    assert_eq!(back.records.len(), interface.records.len());
}

#[test]
fn module_rename_comes_from_the_attribute_args() {
    let args: TokenStream = r#"py(name = "_other")"#.parse().expect("args parse");
    let interface = lower_with(args, "mod sample { }").expect("lowering succeeds");
    assert_eq!(interface.names.py.as_deref(), Some("_other"));
}

#[test]
fn the_record_marker_on_an_enum_points_at_enumeration() {
    let message = error_message("mod m { #[unibind::record] pub enum Kind { A, B } }");
    assert!(message.contains("#[unibind::enumeration]"), "{message}");
}

#[test]
fn unit_enums_lower_with_snake_case_wire_spellings() {
    let interface = lower(
        "mod m {
            /// How a machine is doing.
            #[unibind::enumeration]
            pub enum MachineStatus {
                /// Up and answering.
                Running,
                NotFound,
            }
            pub fn status() -> MachineStatus { MachineStatus::Running }
            pub fn set(status: MachineStatus) {}
        }",
    )
    .expect("unit enums lower");
    let declared = &interface.enums[0];
    assert_eq!(declared.name, "MachineStatus");
    assert_eq!(declared.docs, ["How a machine is doing."]);
    let wires: Vec<&str> = declared
        .variants
        .iter()
        .map(|variant| variant.wire.as_str())
        .collect();
    assert_eq!(wires, ["running", "not_found"]);
    // The Python member identifier is decided once, at lowering, so the two
    // Python renderers cannot derive it differently.
    let members: Vec<Option<&str>> = declared
        .variants
        .iter()
        .map(|variant| variant.names.py.as_deref())
        .collect();
    assert_eq!(members, [Some("RUNNING"), Some("NOT_FOUND")]);
    assert_eq!(declared.variants[0].docs, ["Up and answering."]);
    // Both positions resolve: an enum is owned data, like a record.
    assert!(matches!(
        interface.functions[0].ret,
        Some(ir::Type::Named(ref name)) if name == "MachineStatus"
    ));
    assert!(matches!(
        interface.functions[1].args[0].ty,
        ir::Type::Named(ref name) if name == "MachineStatus"
    ));
}

#[test]
fn rename_all_sets_the_wire_spelling() {
    let interface = lower(
        "mod m {
            #[unibind::enumeration(rename_all = \"PascalCase\")]
            pub enum Kind { PhaseStarted, Done }
            pub fn kind() -> Kind { Kind::Done }
        }",
    )
    .expect("rename_all lowers");
    let wires: Vec<&str> = interface.enums[0]
        .variants
        .iter()
        .map(|variant| variant.wire.as_str())
        .collect();
    assert_eq!(wires, ["PhaseStarted", "Done"]);
}

#[test]
fn an_unknown_rename_all_convention_lists_the_ones_that_exist() {
    let message = error_message(
        "mod m { #[unibind::enumeration(rename_all = \"Train-Case\")] pub enum K { A } }",
    );
    assert!(message.contains("`Train-Case`"), "{message}");
    assert!(message.contains("SCREAMING_SNAKE_CASE"), "{message}");
}

#[test]
fn rename_all_outside_an_enumeration_is_rejected() {
    let message =
        error_message("mod m { #[unibind::record(rename_all = \"snake_case\")] pub struct R {} }");
    assert!(message.contains("`rename_all`"), "{message}");
}

#[test]
fn a_variant_with_data_names_the_variant_and_the_shape() {
    let message = error_message(
        "mod m { #[unibind::enumeration] pub enum Frame { Phase { at: u32 }, Done } }",
    );
    assert!(message.contains("`Frame::Phase`"), "{message}");
    assert!(message.contains("sum type"), "{message}");
    assert!(message.contains("not supported yet"), "{message}");
}

#[test]
fn a_tuple_variant_is_data_too() {
    let message = error_message("mod m { #[unibind::enumeration] pub enum Frame { Phase(u32) } }");
    assert!(message.contains("`Frame::Phase`"), "{message}");
}

#[test]
fn variants_colliding_on_the_wire_are_rejected() {
    let message = error_message(
        "mod m { #[unibind::enumeration(rename_all = \"lowercase\")] pub enum K { AB, Ab } }",
    );
    assert!(message.contains("already claims"), "{message}");
}

#[test]
fn an_enumeration_argument_cannot_carry_a_default() {
    let message = error_message(
        "mod m {
            #[unibind::enumeration] pub enum K { A }
            pub fn go(#[unibind(default = \"a\")] value: K) {}
        }",
    );
    assert!(message.contains("cannot carry a default"), "{message}");
    assert!(message.contains("`K`"), "{message}");
}

#[test]
fn an_empty_enumeration_is_rejected() {
    let message = error_message("mod m { #[unibind::enumeration] pub enum K {} }");
    assert!(message.contains("at least one variant"), "{message}");
}

#[test]
fn an_enumeration_shares_the_type_namespace() {
    let message = error_message(
        "mod m {
            #[unibind::record] pub struct Kind { pub a: u32 }
            #[unibind::enumeration] pub enum Kind { A }
        }",
    );
    assert!(message.contains("declared twice"), "{message}");
}

#[test]
fn the_enumeration_marker_on_a_struct_points_at_record() {
    let message =
        error_message("mod m { #[unibind::enumeration] pub struct K { pub a: u32 } }");
    assert!(message.contains("#[unibind::record]"), "{message}");
}

#[test]
fn unknown_types_name_the_offender() {
    let message = error_message("mod m { pub fn go(value: Mystery) {} }");
    assert!(message.contains("`Mystery`"), "{message}");
}

#[test]
fn required_after_defaulted_is_rejected() {
    let message = error_message("mod m { pub fn go(#[unibind(default = 1)] a: u32, b: u32) {} }");
    assert!(message.contains("needs a default"), "{message}");
}

#[test]
fn foreign_error_types_are_rejected() {
    let message = error_message("mod m { pub fn go() -> Result<(), std::io::Error> { Ok(()) } }");
    assert!(message.contains("#[unibind::error]"), "{message}");
}

#[test]
fn private_record_fields_are_rejected() {
    let message = error_message("mod m { #[unibind::record] pub struct Row { id: u64 } }");
    assert!(message.contains("must be `pub`"), "{message}");
}

#[test]
fn strip_removes_every_unibind_attribute() {
    let file: syn::File = syn::parse_str(FULL).expect("fixture parses");
    let Some(syn::Item::Mod(module)) = file.items.first() else {
        panic!("fixture starts with a module");
    };
    let mut module = module.clone();
    unibind_core::strip_unibind_attrs(&mut module);
    let rendered = quote::quote!(#module).to_string();
    assert!(!rendered.contains("unibind"), "{rendered}");
}

#[test]
fn export_backends_parses_and_rejects() {
    let none = unibind_core::export_backends(TokenStream::new()).expect("empty args parse");
    assert!(none.is_none());

    let args: TokenStream = "backends(py, ts)".parse().expect("tokens");
    let both = unibind_core::export_backends(args)
        .expect("backends list parses")
        .expect("backends listed");
    assert_eq!(both, [unibind_core::Backend::Py, unibind_core::Backend::Ts]);

    let args: TokenStream = "backends(rb)".parse().expect("tokens");
    let error = unibind_core::export_backends(args).expect_err("unknown backend");
    assert!(
        error
            .message
            .contains("expected `py`, `ts`, `ex`, `jvm`, or `wasm`"),
        "{}",
        error.message
    );

    let error = error_message("mod m { pub fn go(#[unibind(backends(py))] value: bool) {} }");
    assert!(error.contains("applies to #[unibind::export]"), "{error}");
}

#[test]
fn ts_renames_lower_into_names() {
    let interface = lower(
        "mod m { #[unibind(ts(name = \"goFast\"))] pub fn go_fast(#[unibind(ts(name = \"theValue\"))] value: bool) { let _ = value; } }",
    )
    .expect("lowers");
    assert_eq!(interface.functions[0].names.ts.as_deref(), Some("goFast"));
    assert_eq!(
        interface.functions[0].args[0].names.ts.as_deref(),
        Some("theValue")
    );
}
