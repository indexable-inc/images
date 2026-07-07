//! Per-export stream classes: which exports return `UniStream` and what
//! the pyo3 backend names the class it wraps each one in. The stub
//! declares exactly those classes, so everything here mirrors
//! `unibind-backend-py`'s `stream.rs`.

use unibind_core::ir;

/// One stream-returning export: the callable that produced it plus the
/// item type its class yields.
pub struct StreamExport<'a> {
    /// `None` for free functions, the owning object's Rust name for
    /// methods; scopes the class name.
    pub owner: Option<&'a str>,
    /// The stream-returning callable.
    pub function: &'a ir::Function,
    /// The yielded item type.
    pub item: &'a ir::Type,
}

/// Every stream-returning export in the interface, in the backend's render
/// order (free functions first, then each object's methods).
pub fn collect(interface: &ir::Interface) -> Vec<StreamExport<'_>> {
    let free = interface
        .functions
        .iter()
        .filter_map(|function| stream_export(None, function));
    let methods = interface.objects.iter().flat_map(|object| {
        object
            .methods
            .iter()
            .filter_map(|method| stream_export(Some(object.name.as_str()), method))
    });
    free.chain(methods).collect()
}

fn stream_export<'a>(
    owner: Option<&'a str>,
    function: &'a ir::Function,
) -> Option<StreamExport<'a>> {
    let Some(ir::Type::Stream(item)) = &function.ret else {
        return None;
    };
    Some(StreamExport {
        owner,
        function,
        item,
    })
}

/// The Python-visible class name the backend registers for one export:
/// `TailStream` for a free `tail`, `StoreWatchStream` for `Store::watch`.
/// Built from the Rust names; renames never reach these classes.
pub fn class_name(owner: Option<&str>, export: &str) -> String {
    let export = pascal_case(export);
    owner.map_or_else(
        || format!("{export}Stream"),
        |object| format!("{object}{export}Stream"),
    )
}

/// `snake_case` -> `PascalCase` for export names.
fn pascal_case(name: &str) -> String {
    name.split('_')
        .map(|segment| {
            let mut chars = segment.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_ascii_uppercase().to_string() + chars.as_str()
            })
        })
        .collect()
}
