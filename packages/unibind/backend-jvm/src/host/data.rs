//! The nested data declarations: one Java `record` per exported record
//! (plus its private wire codecs), one exception hierarchy per error enum
//! (plus its private rebuild factory), and the fixed `PanicException`.

use std::fmt::Write as _;

use heck::ToLowerCamelCase as _;
use unibind_core::ir;
use unibind_core::render::RenderError;

use super::java;
use crate::names;

/// Render every record, exception, and factory declaration.
pub fn render(interface: &ir::Interface) -> Result<String, RenderError> {
    let mut sections = Vec::new();
    for record in &interface.records {
        sections.push(record_decl(record, interface)?);
        sections.push(record_codecs(record, interface)?);
    }
    for error in &interface.errors {
        sections.push(exception_decl(error));
        sections.push(exception_factory(error));
    }
    sections.push(panic_exception());
    Ok(sections.join("\n\n"))
}

/// The public nested `record` mirroring one exported record.
fn record_decl(record: &ir::Record, interface: &ir::Interface) -> Result<String, RenderError> {
    let name = names::record_name(record);
    let mut lines: Vec<String> = record
        .docs
        .iter()
        .map(|line| java::javadoc_escape(line.trim()))
        .collect();
    let documented = record
        .fields
        .iter()
        .filter(|field| !field.docs.is_empty())
        .collect::<Vec<_>>();
    if !documented.is_empty() {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        for field in documented {
            let component = names::component_name(record, field)?;
            let text = field
                .docs
                .iter()
                .map(|line| java::javadoc_escape(line.trim()))
                .collect::<Vec<_>>()
                .join(" ");
            lines.push(format!("@param {component} {text}"));
        }
    }

    let components = record
        .fields
        .iter()
        .map(|field| {
            Ok(format!(
                "{} {}",
                java::declared(&field.ty, interface),
                names::component_name(record, field)?
            ))
        })
        .collect::<Result<Vec<_>, RenderError>>()?
        .join(", ");
    Ok(format!(
        "{javadoc}public record {name}({components}) {{}}",
        javadoc = java::javadoc(&lines),
    ))
}

/// The private wire codecs for one record; fields travel in declaration
/// order with no framing, mirroring the Rust glue's helpers.
fn record_codecs(record: &ir::Record, interface: &ir::Interface) -> Result<String, RenderError> {
    let name = names::record_name(record);

    let decodes = record
        .fields
        .iter()
        .map(|field| java::decode(&field.ty, interface, "reader", 0))
        .collect::<Vec<_>>()
        .join(",\n        ");
    let read = format!(
        "private static {name} read{name}(UnibindReader reader) {{\n    \
         return new {name}(\n        {decodes});\n}}"
    );

    let mut write = format!("private static void write{name}(UnibindWire wire, {name} value) {{\n");
    for field in &record.fields {
        let place = format!("value.{}()", names::component_name(record, field)?);
        let encode = java::encode(&field.ty, interface, &place, "wire", 0);
        let _ = writeln!(write, "{}", java::indent(&encode, 1));
    }
    write.push('}');

    Ok(format!("{read}\n\n{write}"))
}

/// The exception hierarchy for one error enum: a base class extending the
/// `jvm(base = ...)` choice, and one final subclass per variant.
fn exception_decl(error: &ir::ErrorType) -> String {
    let name = names::exception_name(error);
    let base = error.jvm_base.as_deref().unwrap_or("RuntimeException");

    let mut out = String::new();
    let lines: Vec<String> = error
        .docs
        .iter()
        .map(|line| java::javadoc_escape(line.trim()))
        .collect();
    out.push_str(&java::javadoc(&lines));
    let _ = writeln!(out, "public static class {name} extends {base} {{");
    let _ = writeln!(out, "    {name}(String message) {{");
    out.push_str("        super(message);\n    }\n");

    for variant in &error.variants {
        let variant_name = names::variant_exception_name(variant);
        let variant_lines: Vec<String> = variant
            .docs
            .iter()
            .map(|line| java::javadoc_escape(line.trim()))
            .collect();
        out.push('\n');
        // `indent` trims the block's trailing newline; restore it so the
        // class declaration starts on its own line.
        let javadoc = java::indent(&java::javadoc(&variant_lines), 1);
        if !javadoc.is_empty() {
            out.push_str(&javadoc);
            out.push('\n');
        }
        let _ = writeln!(
            out,
            "    public static final class {variant_name} extends {name} {{"
        );
        let _ = writeln!(out, "        {variant_name}(String message) {{");
        out.push_str("            super(message);\n        }\n    }\n");
    }
    out.push('}');
    out
}

/// The private factory rebuilding one error from its wire envelope: the
/// variant's declaration index picks the subclass; an unknown index (a
/// newer native library) falls back to the base class.
fn exception_factory(error: &ir::ErrorType) -> String {
    let name = names::exception_name(error);
    let factory = name.to_lower_camel_case();

    let mut out = format!(
        "private static {name} {factory}(int variant, String message) {{\n    \
         return switch (variant) {{\n"
    );
    for (index, variant) in error.variants.iter().enumerate() {
        let variant_name = names::variant_exception_name(variant);
        let _ = writeln!(
            out,
            "        case {index} -> new {name}.{variant_name}(message);"
        );
    }
    out.push_str("        default -> new ");
    out.push_str(&name);
    out.push_str("(message);\n    };\n}");
    out
}

/// The fixed exception carrying a native-side panic.
fn panic_exception() -> String {
    let javadoc = java::javadoc(&[
        "A panic crossing from the native side; the message carries the".to_owned(),
        "panic text. Always a bug in the native library, never an API".to_owned(),
        "error.".to_owned(),
    ]);
    format!(
        "{javadoc}public static final class PanicException extends RuntimeException {{\n    \
         PanicException(String message) {{\n        super(message);\n    }}\n}}"
    )
}
