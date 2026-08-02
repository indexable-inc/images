//! JSON Schema (draft 2020-12) from [`Ty`]: the second output of the one type
//! language (#4447).
//!
//! [`document`] describes the value a module's `export default` function takes
//! as its argument -- the `params.json` an `ix new <template> <JSON>` caller
//! writes and an editor completes. `type` aliases land in `$defs`, so a named
//! type stays named in the schema too.
//!
//! # What the document describes
//!
//! The schema states **what the annotation asserts**, not what today's checker
//! verifies. The two differ because `ix-ty.nix` stops at weak head normal form
//! (#4451): `listOf string` checks "is a list" and never looks at an element,
//! and `attrs` reads required field *names* and never checks a field's type. So
//! `items`, `additionalProperties`, and `properties` are all **stricter** than
//! the check that runs, deliberately -- `string[]` does mean a list of strings,
//! and a schema that dropped `items` to match the checker's current reach would
//! describe the gap instead of the type.
//!
//! The one thing the schema must not do is assert something the annotation
//! never said. That is why an object type stays open: `{ a: int }` in
//! TypeScript is a lower bound, not a closed record, and `attrs` matches it, so
//! `additionalProperties: false` would be an invention rather than a
//! tightening. Strictness beyond the checker is fine when the annotation says
//! it; strictness the annotation never said is not.
//!
//! # Where the mapping is lossy
//!
//! Four spellings cannot be carried across faithfully: `drv` and function types
//! have no JSON form at all, `path` keeps only its absolute-string branch, and
//! `float` cannot be narrowed away from `int`. Each is decided explicitly
//! below, naming what a consumer gives up, because the failure mode of guessing
//! is silent: a schema wider than the annotation green-lights a params file
//! that then fails at eval, and a narrower one rejects data the module accepts.

use serde_json::{Map, Value, json};

use crate::ty::{Field, Literal, ModuleTypes, Ty};

/// The dialect this crate emits. Named in the document so a validator does not
/// have to guess which draft's semantics apply.
const DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";

/// A module's JSON Schema, pretty-printed with a trailing newline.
///
/// # Panics
///
/// Panics if `serde_json` cannot serialize the assembled document, which no
/// [`ModuleTypes`] can cause: every keyword below is already a
/// [`serde_json::Value`], and the one value that could have been lossy -- a
/// non-finite float literal, which serializes to `null` without erroring -- is
/// rejected at parse time by [`crate::map`].
#[must_use]
pub fn document(types: &ModuleTypes) -> String {
    // The root constrains the argument of `export default`. Nix functions
    // curry and a JSON document describes one value, so a curried module's
    // trailing parameters have no place here: a consumer sees the first
    // parameter only. That is the right shape for the case this exists for --
    // a template takes one params record -- and a module wanting more has to
    // say so by taking a record.
    //
    // With no annotated parameter to describe, the root stays the empty schema:
    // "nothing is known about this argument", which is not the same claim as
    // "this module takes nothing".
    let mut root = types
        .parameters
        .first()
        .and_then(|parameter| parameter.ty.as_ref())
        .map_or_else(Map::new, schema);

    root.insert("$schema".to_owned(), DIALECT.into());

    if !types.aliases.is_empty() {
        // Every alias, not only the ones the root reaches. An alias is part of
        // the module's declared surface, and pruning to reachable ones would
        // hide exactly the alias a reader went looking for.
        let defs = types
            .aliases
            .iter()
            .map(|alias| (alias.name.clone(), Value::Object(schema(&alias.ty))))
            .collect();
        root.insert("$defs".to_owned(), Value::Object(defs));
    }

    let mut out = serde_json::to_string_pretty(&sorted(Value::Object(root)))
        .expect("an assembled schema is finite JSON");
    out.push('\n');
    out
}

/// `value` with every object's keys in sorted order.
///
/// These documents are pinned byte for byte by `tests/golden/*.schema.golden`,
/// so their key order has to be a property of this crate and not of whoever
/// else happens to be in the build. Without this it is the latter:
/// `serde_json::Map` is a `BTreeMap` normally and an `IndexMap` when
/// `serde_json`'s `preserve_order` feature is on, and a feature is on for
/// everyone once anyone asks for it. `packages/index-delta` asks for it today,
/// so a build that resolves features across the whole workspace flips this
/// crate's output from sorted to insertion order while nothing in this crate
/// changes.
///
/// Re-inserting in sorted order gives the same bytes under either backing map:
/// a `BTreeMap` was already sorted, and an `IndexMap` iterates in the order it
/// was filled.
fn sorted(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
            entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, sorted(value)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
        other => other,
    }
}

/// Builds a schema object from its keywords.
fn keywords<const N: usize>(pairs: [(&str, Value); N]) -> Map<String, Value> {
    pairs
        .into_iter()
        .map(|(keyword, value)| (keyword.to_owned(), value))
        .collect()
}

/// The schema keywords for one type.
fn schema(ty: &Ty) -> Map<String, Value> {
    match ty {
        // `any`/`unknown` check nothing and the empty schema accepts
        // everything: the two agree exactly.
        Ty::Any => Map::new(),
        Ty::Str => keywords([("type", "string".into())]),
        Ty::Bool => keywords([("type", "boolean".into())]),
        Ty::Int => keywords([("type", "integer".into())]),
        // Nix splits int from float; JSON has one number type, and draft
        // 2020-12 counts `2.0` as an integer. `number` is therefore LOOSER
        // than `__ixTy.float`, which rejects an int outright. The strict
        // spelling (`"not": { "type": "integer" }`) would reject `2.0`, a
        // perfectly good Nix float, so the schema stays loose: a consumer
        // gets no editor warning that `1` is not a float and learns it from
        // the eval-time check instead.
        Ty::Float => keywords([("type", "number".into())]),
        Ty::Ranged(range) => keywords([
            ("type", "integer".into()),
            ("minimum", range.min.into()),
            ("maximum", range.max.into()),
        ]),
        // A Nix path VALUE cannot appear in a JSON document at all, so the
        // only spelling left is the other branch `ix-ty.nix`'s `path` accepts:
        // an absolute path as a string. A consumer loses relative paths (which
        // would need the module's directory, which JSON has no way to carry)
        // and store-path string context.
        Ty::Path => keywords([("type", "string".into()), ("pattern", "^/".into())]),
        Ty::NonEmptyStr => keywords([("type", "string".into()), ("minLength", 1.into())]),
        // A derivation exists only inside an evaluation, so no JSON value can
        // produce one, and saying so is better than widening to `object` and
        // letting an editor suggest a shape that cannot work.
        Ty::Drv => unrepresentable("a derivation exists only during evaluation; supply it from Nix, not from JSON"),
        // Same verdict, same reason: a function is code, not data.
        Ty::Func => unrepresentable("a function has no JSON form; supply it from Nix, not from JSON"),
        Ty::List(item) => keywords([
            ("type", "array".into()),
            ("items", Value::Object(schema(item))),
        ]),
        Ty::Attrs(value) => keywords([
            ("type", "object".into()),
            ("additionalProperties", Value::Object(schema(value))),
        ]),
        Ty::Object(fields) => object(fields),
        Ty::Enum(literals) => keywords([(
            "enum",
            literals.iter().map(literal).collect::<Vec<Value>>().into(),
        )]),
        // `anyOf` rather than `"type": [T, "null"]`: the inner schema can
        // carry keywords (`minimum`, `pattern`, `$ref`) that a type array has
        // nowhere to put.
        Ty::Nullable(inner) => keywords([(
            "anyOf",
            json!([Value::Object(schema(inner)), { "type": "null" }]),
        )]),
        // Draft 2020-12 evaluates `$ref` alongside its siblings, so the root
        // can be a bare reference next to `$schema` and `$defs`. Alias names
        // are TypeScript identifiers, so no JSON-pointer escaping applies.
        Ty::Alias(name) => keywords([("$ref", format!("#/$defs/{name}").into())]),
    }
}

fn object(fields: &[Field]) -> Map<String, Value> {
    let properties = fields
        .iter()
        .map(|field| (field.name.clone(), Value::Object(schema(&field.ty))))
        .collect();
    let required: Vec<Value> = fields
        .iter()
        .filter(|field| !field.optional)
        .map(|field| field.name.clone().into())
        .collect();

    // No `additionalProperties: false`: a TypeScript object type is a lower
    // bound, and `ix-ty.nix`'s `attrs` checker agrees, so closing the record
    // would assert something neither the annotation nor the runtime says. Note
    // this is not the same call as dropping `items` from a list would be --
    // see the module header on strictness the annotation did say.
    let mut out = keywords([
        ("type", "object".into()),
        ("properties", Value::Object(properties)),
    ]);
    if !required.is_empty() {
        out.insert("required".to_owned(), required.into());
    }
    out
}

/// A schema nothing validates against, carrying the reason.
///
/// `{ "not": {} }` is the empty schema's complement, so it is the same verdict
/// as the boolean schema `false`. `false` is spelled shorter but carries no
/// `description`, and the person who needs to know WHY a field cannot be
/// filled is the one looking at their editor's error on it.
fn unrepresentable(why: &str) -> Map<String, Value> {
    keywords([("not", json!({})), ("description", why.into())])
}

fn literal(literal: &Literal) -> Value {
    match literal {
        Literal::Str(text) => text.clone().into(),
        Literal::Int(value) => (*value).into(),
        // `serde_json` maps a non-finite float to `null` rather than failing,
        // which would put a silent `null` in an `enum`. Safe only because
        // [`crate::map`] refuses a literal that overflows to infinity.
        Literal::Float(value) => json!(value),
        Literal::Bool(value) => (*value).into(),
    }
}
