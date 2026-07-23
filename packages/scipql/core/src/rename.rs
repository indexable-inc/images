//! The built-in `rename` query: rewrite every occurrence of a symbol.
//!
//! This is just a `fix` program generated in Rust, so renaming has no special
//! path: it selects occurrences whose moniker contains `selector` and replaces
//! each occurrence range (the identifier) with `new_name`. Because selection is
//! by SCIP moniker, renaming `net/Socket#` leaves a `mock/Socket#` untouched.

/// Escape a string for a Soufflé `"..."` literal.
fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// A `fix` program that renames occurrences whose moniker *ends with*
/// `selector` to `new_name`.
///
/// Suffix, not substring: SCIP descriptors nest as a path, so the struct
/// `…net/Socket#` is a suffix of its field `…net/Socket#fd.`. Matching the
/// trailing descriptor renames the struct and its references without touching
/// its members. For anything more selective, write a `fix` program directly.
#[must_use]
pub fn program(selector: &str, new_name: &str) -> String {
    let selector = escape(selector);
    format!(
        "edit(path, start, end, \"{new}\") :-\n  \
         occurrence(symbol, path, start, end, _),\n  \
         strlen(symbol) >= strlen(\"{selector}\"),\n  \
         substr(symbol, strlen(symbol) - strlen(\"{selector}\"), strlen(\"{selector}\")) = \"{selector}\".\n",
        new = escape(new_name),
    )
}
