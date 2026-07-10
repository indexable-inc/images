//! Shared assertions for Unibind integration and snapshot tests.

use std::fmt::Write as _;

pub struct RecordAttributes<'a> {
    pub name: &'a str,
    pub outer: String,
    pub fields: Vec<(&'a str, String)>,
}

pub fn format_record_attributes<'a>(records: impl IntoIterator<Item = RecordAttributes<'a>>) -> String {
    let mut shown = String::new();
    for record in records {
        writeln!(shown, "// struct {}: {}", record.name, record.outer).expect("write to string");
        for (name, attributes) in record.fields {
            writeln!(shown, "//   field {name}: {attributes}").expect("write to string");
        }
    }
    shown.push('\n');
    shown
}

#[macro_export]
macro_rules! assert_render_snapshot {
    ($interface:expr, $rendered:expr, $expected:expr, $name:expr) => {{
        let interface = &$interface;
        let rendered = $rendered;
        let records = interface.records.iter().zip(&rendered.records).map(|(record, attrs)| {
            let outer = &attrs.outer;
            $crate::RecordAttributes {
                name: &record.name,
                outer: quote::quote!(#(#outer)*).to_string(),
                fields: record.fields.iter().zip(&attrs.fields).map(|(field, field_attrs)| {
                    (&*field.name, quote::quote!(#(#field_attrs)*).to_string())
                }).collect(),
            }
        });
        let mut shown = $crate::format_record_attributes(records);
        let glue: syn::File = syn::parse2(rendered.glue).expect("glue parses");
        shown.push_str(&prettyplease::unparse(&glue));
        $crate::assert_snapshot(&shown, $expected, $name);
    }};
}

/// Lower a fixture module's source to its interface: parse, take the first
/// item (the exported module), and run the shared lowering. Every backend's
/// snapshot test starts from this same seam.
///
/// # Panics
/// Panics when `source` is not a parseable file starting with a module, or
/// when the module fails to lower — fixture bugs, not runtime conditions.
pub fn lower_module_source(source: &str) -> unibind_core::ir::Interface {
    let file: syn::File = syn::parse_str(source).expect("module parses");
    let Some(syn::Item::Mod(module)) = file.items.first() else {
        panic!("source starts with a module");
    };
    unibind_core::lower_module(proc_macro2::TokenStream::new(), module).expect("module lowers")
}

/// Snapshot an interface's IR as pretty JSON; every backend's render test
/// pins its fixture's lowering this way.
///
/// # Panics
/// Panics when the IR fails to serialize or the snapshot drifted.
pub fn assert_ir_json_snapshot(
    interface: &unibind_core::ir::Interface,
    expected: &str,
    name: &str,
) {
    let json = serde_json::to_string_pretty(interface).expect("serializes");
    assert_snapshot(&json, expected, name);
}

/// Compare rendered output while printing a copy-ready replacement on drift.
///
/// # Panics
/// Panics when `actual` and `expected` differ after trimming outer whitespace.
pub fn assert_snapshot(actual: &str, expected: &str, name: &str) {
    if actual.trim() == expected.trim() {
        return;
    }
    println!("=== actual {name} ===");
    println!("{actual}");
    println!("=== end {name} ===");
    panic!("{name} drifted; copy the printed block into tests/snapshots/{name}");
}
