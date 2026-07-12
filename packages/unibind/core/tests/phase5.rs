//! Lowering of the phase 5 addition: per-language `ex` renames, additive
//! next to the `py` ones. Everything else phase 5 consumes (async,
//! `blocking`, streams, objects) lowered with phase 2 and is covered by
//! `lower_phase2.rs`.

use proc_macro2::TokenStream;
use unibind_core::ir;

fn lower_with(args: TokenStream, source: &str) -> Result<ir::Interface, unibind_core::LowerError> {
    let file: syn::File = syn::parse_str(source).expect("fixture parses");
    let Some(syn::Item::Mod(module)) = file.items.first() else {
        panic!("fixture starts with a module");
    };
    unibind_core::lower_module(args, module)
}

#[test]
fn ex_renames_flow_into_names() {
    let args: TokenStream = r#"ex(name = "Other")"#.parse().expect("args parse");
    let interface = lower_with(
        args,
        r#"
        mod sample {
            #[unibind::record(ex(name = "Line"))]
            #[derive(Clone)]
            pub struct Row {
                #[unibind(ex(name = "tag"))]
                pub name: String,
            }

            #[unibind::error(ex(name = "Fault"))]
            pub enum RowError {
                #[unibind(ex(name = "missing_store"))]
                StoreGone { message: String },
            }

            #[unibind(ex(name = "fetch_rows"))]
            pub fn rows(#[unibind(ex(name = "how_many"))] limit: u64) -> Result<Vec<Row>, RowError> {
                let _ = limit;
                Ok(Vec::new())
            }
        }
        "#,
    )
    .expect("lowering succeeds");
    assert_eq!(interface.names.ex.as_deref(), Some("Other"));
    assert_eq!(interface.names.py, None);
    assert_eq!(interface.records[0].names.ex.as_deref(), Some("Line"));
    assert_eq!(
        interface.records[0].fields[0].names.ex.as_deref(),
        Some("tag")
    );
    assert_eq!(interface.errors[0].names.ex.as_deref(), Some("Fault"));
    assert_eq!(
        interface.errors[0].variants[0].names.ex.as_deref(),
        Some("missing_store")
    );
    let rows = &interface.functions[0];
    assert_eq!(rows.names.ex.as_deref(), Some("fetch_rows"));
    assert_eq!(rows.args[0].names.ex.as_deref(), Some("how_many"));
}

#[test]
fn ex_and_py_renames_coexist() {
    let interface = lower_with(
        TokenStream::new(),
        r#"
        mod m {
            #[unibind(py(name = "count"), ex(name = "tally"))]
            pub fn n() -> u64 {
                0
            }
        }
        "#,
    )
    .expect("lowering succeeds");
    assert_eq!(interface.functions[0].names.py.as_deref(), Some("count"));
    assert_eq!(interface.functions[0].names.ex.as_deref(), Some("tally"));
}

#[test]
fn duplicate_ex_renames_are_rejected() {
    let error = lower_with(
        TokenStream::new(),
        r#"
        mod m {
            #[unibind(ex(name = "a"))]
            #[unibind(ex(name = "b"))]
            pub fn f() {}
        }
        "#,
    )
    .expect_err("duplicate renames fail");
    assert!(error.message.contains("duplicate"), "{}", error.message);
}

#[test]
fn export_backends_parse_and_validate() {
    let args: TokenStream = "backends(ex, py)".parse().expect("args parse");
    let backends = unibind_core::export_backends(args)
        .expect("parses")
        .expect("present");
    assert_eq!(
        backends,
        [unibind_core::Backend::Ex, unibind_core::Backend::Py]
    );

    let absent: TokenStream = r#"py(name = "m")"#.parse().expect("args parse");
    assert_eq!(unibind_core::export_backends(absent).expect("parses"), None);

    let unknown: TokenStream = "backends(zig)".parse().expect("args parse");
    let error = unibind_core::export_backends(unknown).expect_err("unknown backend fails");
    assert!(error.message.contains("unknown backend"), "{}", error.message);

    let misplaced = lower_with(
        TokenStream::new(),
        "mod m { #[unibind(backends(py))] pub fn f() {} }",
    )
    .expect_err("backends on a fn fails");
    assert!(
        misplaced.message.contains("applies to #[unibind::export]"),
        "{}",
        misplaced.message
    );
}
