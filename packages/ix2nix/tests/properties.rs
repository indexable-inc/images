//! Property tests over the converter (hegel, Hypothesis-based).
//!
//! Three claims, each over generated input rather than examples:
//! robustness (arbitrary text never panics the converter), determinism
//! (same source, same bytes out), and the round-trip that matters for a
//! transpiler: every well-formed `.ix` program the generator can spell
//! converts, and the emitted Nix reparses cleanly under an independent
//! parser (rnix). The generator favors small programs; hegel shrinks any
//! counterexample toward the minimal one.

use hegel::TestCase;
use hegel::generators::{self as gs, Generator};
use ix2nix::convert;

// --- structured program generator -----------------------------------------

/// Identifiers kept distinct from type names so shadowing rules never make
/// a generated program ill-formed by accident.
fn ident() -> impl Generator<String> {
    gs::sampled_from(&["a", "b", "c", "d0", "veryLongName"][..]).map(str::to_owned)
}

fn type_name() -> impl Generator<String> {
    gs::sampled_from(
        &[
        "string",
        "bool",
        "int",
        "float",
        "u8",
        "u16",
        "u32",
        "i8",
        "i16",
        "i32",
        "port",
        "path",
        "nonEmptyStr",
        "drv",
        "any",
        "unknown",
        "object",
        ][..],
    )
    .map(str::to_owned)
}

/// A type expression from the supported surface: builtins, arrays,
/// `Record<string, T>`, `T | null`, literal unions, inline object types.
fn ty() -> impl Generator<String> {
    let node = gs::deferred::<String>();
    let handle = node.generator();

    let literal_union = gs::vecs(gs::sampled_from(&["\"tcp\"", "\"udp\"", "1", "3"][..]))
        .min_size(1)
        .max_size(3)
        .map(|lits| lits.join(" | "));
    let array = node.generator().map(|t| format!("({t})[]"));
    let record = node.generator().map(|t| format!("Record<string, {t}>"));
    let nullable = node.generator().map(|t| format!("({t}) | null"));
    let object = gs::vecs(hegel::tuples!(ident(), gs::booleans(), node.generator()))
        .max_size(3)
        .map(|fields| {
            let mut out = String::from("{ ");
            let mut seen: Vec<String> = Vec::new();
            for (name, optional, t) in fields {
                if seen.contains(&name) {
                    continue; // duplicate keys are ill-formed on purpose
                }
                let marker = if optional { "?" } else { "" };
                out.push_str(&format!("{name}{marker}: {t}; "));
                seen.push(name);
            }
            out.push('}');
            out
        });

    node.set(hegel::one_of!(
        type_name(),
        literal_union,
        array,
        record,
        nullable,
        object
    ));
    handle
}

/// An expression tree from the mapped `.ix` surface.
fn expr() -> impl Generator<String> {
    let node = gs::deferred::<String>();
    let handle = node.generator();

    let leaf = hegel::one_of!(
        gs::integers::<i32>().map(|n| n.to_string()),
        gs::sampled_from(
            // The last two exercise the renderer's escaping: a real template
            // interpolation, and a plain string containing literal `${`.
            &["true", "false", "null", "\"s\"", "`t ${a} x`", "\"has \\${ curly\""][..],
        )
        .map(str::to_owned),
        ident()
    );
    let array = gs::vecs(node.generator())
        .max_size(3)
        .map(|items| format!("[{}]", items.join(", ")));
    let object = gs::vecs(hegel::tuples!(ident(), node.generator()))
        .max_size(3)
        .map(|props| {
            let mut out = String::from("{ ");
            let mut seen: Vec<String> = Vec::new();
            for (key, value) in props {
                if seen.contains(&key) {
                    continue;
                }
                out.push_str(&format!("{key}: {value}, "));
                seen.push(key);
            }
            out.push('}');
            out
        });
    let arrow = hegel::tuples!(ident(), gs::optional(ty()), node.generator()).map(
        |(param, annotation, body)| {
            let annotation = annotation.map_or(String::new(), |t| format!(": {t}"));
            // Parenthesized body: a bare `=> {...}` parses as a block, not an
            // object literal.
            format!("(({param}{annotation}) => ({body}))")
        },
    );
    let call = hegel::tuples!(ident(), node.generator()).map(|(f, a)| format!("{f}({a})"));
    let binary = hegel::tuples!(node.generator(), gs::sampled_from(&["+", "==", "&&"][..]), node.generator())
        .map(|(l, op, r)| format!("({l} {op} {r})"));
    let ternary = hegel::tuples!(node.generator(), node.generator(), node.generator())
        .map(|(c, t, e)| format!("({c} ? {t} : {e})"));
    let member = hegel::tuples!(ident(), ident()).map(|(base, field)| format!("{base}.{field}"));
    let cast = hegel::tuples!(node.generator(), ty()).map(|(e, t)| format!("({e} as ({t}))"));

    node.set(hegel::one_of!(
        leaf, array, object, arrow, call, binary, ternary, member, cast
    ));
    handle
}

/// A whole module: optional type alias, a few consts (possibly annotated),
/// one `export default`.
fn program() -> impl Generator<String> {
    hegel::tuples!(
        gs::booleans(),
        gs::vecs(hegel::tuples!(gs::optional(ty()), expr())).max_size(3),
        expr()
    )
    .map(|(with_alias, consts, default)| {
        let mut out = String::new();
        if with_alias {
            out.push_str("type T0 = { host: string; port?: port };\n");
        }
        // Fixed distinct names keep generated `const`s well-formed; values
        // and annotations carry the randomness.
        for (index, (annotation, value)) in consts.into_iter().enumerate() {
            let annotation = annotation.map_or(String::new(), |t| format!(": {t}"));
            out.push_str(&format!("const k{index}{annotation} = {value};\n"));
        }
        out.push_str(&format!("export default {default};\n"));
        out
    })
}

// --- properties ------------------------------------------------------------

#[hegel::test]
fn convert_never_panics_on_arbitrary_text(tc: TestCase) {
    let source: String = tc.draw(gs::text());
    let _ = convert(&source); // Ok or a positioned Err; a panic fails the test
}

#[hegel::test]
fn convert_is_deterministic(tc: TestCase) {
    let source = tc.draw(program());
    assert_eq!(convert(&source), convert(&source));
}

#[hegel::test]
fn well_formed_programs_convert_and_emitted_nix_reparses(tc: TestCase) {
    let source = tc.draw(program());
    let nix = convert(&source)
        .unwrap_or_else(|error| panic!("generated program failed to convert: {error}\n{source}"));
    let parse = rnix::Root::parse(&nix);
    assert!(
        parse.errors().is_empty(),
        "emitted Nix does not reparse: {:?}\n--- source\n{source}\n--- emitted\n{nix}",
        parse.errors()
    );
}
