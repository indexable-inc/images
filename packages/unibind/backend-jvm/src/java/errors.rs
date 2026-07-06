//! One exception hierarchy file per error enum, plus the shared panic
//! exception.
//!
//! The base class carries a protected constructor: only the generated
//! module class (via the variant subclasses) raises these.

use unibind_core::ir;

use crate::java::{line, types};
use crate::names;

pub fn render(interface: &ir::Interface, error: &ir::ErrorType) -> String {
    let mut out = format!("package {};\n\n", names::java_package(&interface.name));
    out.push_str(&types::doc_block(&error.docs, 0));
    let name = format!("{}Exception", error.name);
    line(&mut out, 0, &format!("public class {name} extends RuntimeException {{"));
    line(&mut out, 0, "");
    line(&mut out, 1, &format!("protected {name}(String message) {{"));
    line(&mut out, 2, "super(message);");
    line(&mut out, 1, "}");
    for variant in &error.variants {
        line(&mut out, 0, "");
        out.push_str(&types::doc_block(&variant.docs, 1));
        line(
            &mut out,
            1,
            &format!("public static final class {} extends {name} {{", variant.name),
        );
        line(&mut out, 0, "");
        line(&mut out, 2, &format!("public {}(String message) {{", variant.name));
        line(&mut out, 3, "super(message);");
        line(&mut out, 2, "}");
        line(&mut out, 1, "}");
    }
    line(&mut out, 0, "}");
    out
}

/// The per-module panic exception (envelope code -1). Generated per module
/// so no shared runtime jar exists yet.
pub fn panic_exception(interface: &ir::Interface) -> String {
    let mut out = format!("package {};\n\n", names::java_package(&interface.name));
    let doc = vec!["A Rust panic crossed the unibind boundary (envelope code -1).".to_owned()];
    out.push_str(&types::doc_block(&doc, 0));
    line(
        &mut out,
        0,
        "public final class UnibindPanicException extends RuntimeException {",
    );
    line(&mut out, 0, "");
    line(&mut out, 1, "public UnibindPanicException(String message) {");
    line(&mut out, 2, "super(message);");
    line(&mut out, 1, "}");
    line(&mut out, 0, "}");
    out
}
