//! Lowering across the phase 0 surface: the golden IR for a full-featured
//! module, and the positioned errors for everything out of scope.

use proc_macro2::TokenStream;
use unibind_core::ir;

fn lower(source: &str) -> Result<ir::Interface, unibind_core::LowerError> {
    lower_with(TokenStream::new(), source)
}

fn lower_with(
    args: TokenStream,
    source: &str,
) -> Result<ir::Interface, unibind_core::LowerError> {
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
    assert_eq!(error.variants[0].names.py.as_deref(), Some("StoreGoneError"));
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
fn data_enums_are_rejected() {
    let message = error_message("mod m { #[unibind::record] pub enum Kind { A, B } }");
    assert!(message.contains("data enums"), "{message}");
}

#[test]
fn unknown_types_name_the_offender() {
    let message = error_message("mod m { pub fn go(value: Mystery) {} }");
    assert!(message.contains("`Mystery`"), "{message}");
}

#[test]
fn required_after_defaulted_is_rejected() {
    let message =
        error_message("mod m { pub fn go(#[unibind(default = 1)] a: u32, b: u32) {} }");
    assert!(message.contains("needs a default"), "{message}");
}

#[test]
fn foreign_error_types_are_rejected() {
    let message =
        error_message("mod m { pub fn go() -> Result<(), std::io::Error> { Ok(()) } }");
    assert!(message.contains("#[unibind::error]"), "{message}");
}

#[test]
fn private_record_fields_are_rejected() {
    let message =
        error_message("mod m { #[unibind::record] pub struct Row { id: u64 } }");
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
        error.message.contains("expected `py`, `ts`, or `ex`"),
        "{}",
        error.message
    );

    let error = error_message(
        "mod m { pub fn go(#[unibind(backends(py))] value: bool) {} }",
    );
    assert!(error.contains("applies to #[unibind::export]"), "{error}");
}

#[test]
fn ts_renames_lower_into_names() {
    let interface = lower(
        "mod m { #[unibind(ts(name = \"goFast\"))] pub fn go_fast(#[unibind(ts(name = \"theValue\"))] value: bool) { let _ = value; } }",
    )
    .expect("lowers");
    assert_eq!(interface.functions[0].names.ts.as_deref(), Some("goFast"));
    assert_eq!(interface.functions[0].args[0].names.ts.as_deref(), Some("theValue"));
}
