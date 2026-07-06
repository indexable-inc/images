//! Assemble the module class: library loading, downcall handles, the ABI
//! probe, the public methods, and the private encode/decode helpers.

use unibind_core::ir;

use crate::ctype::CTy;
use crate::java::{decode, decode_set, encode, encode_set, line, methods, types};
use crate::model::Model;
use crate::{names, RenderError};

pub fn render(interface: &ir::Interface, model: &Model<'_>) -> Result<String, RenderError> {
    let class = names::pascal(&interface.name);
    let module = &interface.name;
    let mut out = format!("package {};\n\n", names::java_package(module));
    for import in [
        "java.lang.foreign.Arena",
        "java.lang.foreign.FunctionDescriptor",
        "java.lang.foreign.Linker",
        "java.lang.foreign.MemorySegment",
        "java.lang.foreign.SymbolLookup",
        "java.lang.foreign.ValueLayout",
        "java.lang.invoke.MethodHandle",
        "java.nio.charset.StandardCharsets",
    ] {
        line(&mut out, 0, &format!("import {import};"));
    }
    line(&mut out, 0, "");

    let mut doc = interface.docs.clone();
    if !doc.is_empty() {
        doc.push(String::new());
    }
    doc.push(format!(
        "Java binding for the Rust module {module}; loads the native library named by the"
    ));
    doc.push(format!("{{@code unibind.{module}.library}} system property."));
    out.push_str(&types::doc_block(&doc, 0));
    line(&mut out, 0, &format!("public final class {class} {{"));
    line(&mut out, 0, "");
    line(&mut out, 1, &format!("private {class}() {{"));
    line(&mut out, 1, "}");
    line(&mut out, 0, "");
    line(&mut out, 1, "private static final SymbolLookup LOOKUP = loadLibrary();");
    line(&mut out, 1, "private static final Linker LINKER = Linker.nativeLinker();");
    for function in &interface.functions {
        handles(&mut out, interface, function);
    }
    line(&mut out, 0, "");
    abi_check(&mut out, module);

    out.push_str(&methods::all(interface, model)?);
    out.push_str(&encode::helpers(model, &encode_set(interface, model)));
    out.push_str(&decode::helpers(model, &decode_set(interface, model)));
    infrastructure(&mut out, module);
    line(&mut out, 0, "}");
    Ok(out)
}

/// The call handle and free handle for one export.
fn handles(out: &mut String, interface: &ir::Interface, function: &ir::Function) {
    let handle = names::handle_const(&function.name);
    let symbol = names::export_symbol(&interface.name, &function.name);
    let free = names::free_symbol(&interface.name, &function.name);
    line(&mut *out, 1, &format!("private static final MethodHandle {handle} = handle("));
    line(&mut *out, 3, &format!("\"{symbol}\","));
    if function.args.is_empty() {
        line(&mut *out, 3, "FunctionDescriptor.of(ValueLayout.ADDRESS));");
    } else {
        line(&mut *out, 3, "FunctionDescriptor.of(");
        line(&mut *out, 5, "ValueLayout.ADDRESS,");
        let layouts: Vec<String> = function
            .args
            .iter()
            .map(|arg| format!("                    {}", types::value_layout(&CTy::of(&arg.ty))))
            .collect();
        out.push_str(&layouts.join(",\n"));
        out.push_str("));\n");
    }
    line(&mut *out, 1, &format!("private static final MethodHandle {handle}_FREE = handle("));
    line(&mut *out, 3, &format!("\"{free}\", FunctionDescriptor.ofVoid(ValueLayout.ADDRESS));"));
}

fn abi_check(out: &mut String, module: &str) {
    let symbol = names::abi_symbol(module);
    line(&mut *out, 1, "static {");
    line(
        &mut *out,
        2,
        &format!("MethodHandle abi = handle(\"{symbol}\", FunctionDescriptor.of(ValueLayout.JAVA_INT));"),
    );
    line(&mut *out, 2, "int version;");
    line(&mut *out, 2, "try {");
    line(&mut *out, 3, "version = (int) abi.invokeExact();");
    line(&mut *out, 2, "} catch (Throwable error) {");
    line(&mut *out, 3, "throw new IllegalStateException(\"unibind ABI probe failed\", error);");
    line(&mut *out, 2, "}");
    line(&mut *out, 2, "if (version != 0) {");
    line(
        &mut *out,
        3,
        "throw new IllegalStateException(",
    );
    line(
        &mut *out,
        5,
        "\"native library speaks unibind ABI \" + version + \"; this binding expects 0\");",
    );
    line(&mut *out, 2, "}");
    line(&mut *out, 1, "}");
}

fn infrastructure(out: &mut String, module: &str) {
    line(&mut *out, 0, "");
    line(&mut *out, 1, "private static SymbolLookup loadLibrary() {");
    line(
        &mut *out,
        2,
        &format!("String library = System.getProperty(\"unibind.{module}.library\");"),
    );
    line(&mut *out, 2, "if (library == null) {");
    line(&mut *out, 3, "throw new IllegalStateException(");
    line(
        &mut *out,
        5,
        &format!(
            "\"set -Dunibind.{module}.library=/path/to/the/native/library before using this binding\");"
        ),
    );
    line(&mut *out, 2, "}");
    line(
        &mut *out,
        2,
        "return SymbolLookup.libraryLookup(java.nio.file.Path.of(library), Arena.global());",
    );
    line(&mut *out, 1, "}");
    line(&mut *out, 0, "");
    line(
        &mut *out,
        1,
        "private static MethodHandle handle(String symbol, FunctionDescriptor descriptor) {",
    );
    line(&mut *out, 2, "MemorySegment address = LOOKUP.find(symbol)");
    line(
        &mut *out,
        4,
        ".orElseThrow(() -> new IllegalStateException(\"native library exports no \" + symbol));",
    );
    line(&mut *out, 2, "return LINKER.downcallHandle(address, descriptor);");
    line(&mut *out, 1, "}");
    line(&mut *out, 0, "");
    line(&mut *out, 1, "private static void free(MethodHandle handle, MemorySegment envelope) {");
    line(&mut *out, 2, "try {");
    line(&mut *out, 3, "handle.invokeExact(envelope);");
    line(&mut *out, 2, "} catch (Throwable error) {");
    line(&mut *out, 3, "throw new IllegalStateException(\"unibind envelope free failed\", error);");
    line(&mut *out, 2, "}");
    line(&mut *out, 1, "}");
}
