//! One Java `record` file per IR record.

use unibind_core::ir;

use crate::ctype::CTy;
use crate::java::{line, types};
use crate::names;

pub fn render(interface: &ir::Interface, record: &ir::Record) -> String {
    let mut out = format!("package {};\n\n", names::java_package(&interface.name));

    let mut doc = record.docs.clone();
    if !record.fields.is_empty() {
        if !doc.is_empty() {
            doc.push(String::new());
        }
        for field in &record.fields {
            let cty = CTy::of(&field.ty);
            let mut text = format!("@param {}", names::camel(&field.name));
            for docs_line in &field.docs {
                text.push(' ');
                text.push_str(docs_line);
            }
            for note in types::doc_notes(&cty) {
                text.push(' ');
                text.push_str(note);
            }
            doc.push(text);
        }
    }
    out.push_str(&types::doc_block(&doc, 0));

    if record.fields.is_empty() {
        line(&mut out, 0, &format!("public record {}() {{", record.name));
    } else {
        line(&mut out, 0, &format!("public record {}(", record.name));
        let components: Vec<String> = record
            .fields
            .iter()
            .map(|field| {
                format!(
                    "        {} {}",
                    types::java_type(&CTy::of(&field.ty), false),
                    names::camel(&field.name)
                )
            })
            .collect();
        out.push_str(&components.join(",\n"));
        out.push_str(") {\n");
    }
    out.push_str("}\n");
    out
}
