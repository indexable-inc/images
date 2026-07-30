//! Behavior tests across the public boundary: `.ix` source in, Nix source or
//! a positioned diagnostic out.

use ix2nix::convert;

fn nix(source: &str) -> String {
    let out = convert(source).expect("source should convert");
    out.strip_prefix("{ __dir, __importIx, __ixTy }:\n")
        .expect("every module renders under the wrapper")
        .to_owned()
}

fn diagnostic(source: &str) -> ix2nix::Error {
    convert(source).expect_err("source should be rejected")
}

// --- types ---

#[test]
fn parameter_annotation_lowers_to_arg_check() {
    let out = nix("export default (a: string) => a;\n");
    assert_eq!(out, "a: __ixTy.arg \"1:17 argument `a`\" __ixTy.string a a\n");
}

#[test]
fn return_annotation_wraps_the_innermost_body() {
    let out = nix("export default (a, b): int => a;\n");
    assert_eq!(out, "a: b: __ixTy.ret \"1:22 return\" __ixTy.int a\n");
}

#[test]
fn as_cast_checks_but_as_unknown_erases() {
    let out = nix("export default x as int;\n");
    assert_eq!(out, "__ixTy.ret \"1:21 as\" __ixTy.int x\n");
    assert_eq!(nix("export default x as unknown;\n"), "x\n");
    assert_eq!(nix("export default x as any;\n"), "x\n");
}

#[test]
fn type_alias_becomes_a_checker_binding() {
    let out = nix("type P = \"a\" | \"b\";\nexport default (x: P) => x;\n");
    assert!(out.contains("ty'P = __ixTy.enum [ \"a\" \"b\" ]"), "{out}");
    assert!(out.contains("__ixTy.arg \"2:17 argument `x`\" ty'P x"), "{out}");
}

#[test]
fn nullable_union_lowers_to_nullable() {
    let out = nix("export default (x: int | null) => x;\n");
    assert!(out.contains("__ixTy.nullable __ixTy.int"), "{out}");
}

#[test]
fn destructured_parameter_checks_bound_fields() {
    let out = nix("export default ({ a }: { a: int }) => a;\n");
    assert_eq!(
        out,
        "{ a }: __ixTy.arg \"1:26 argument field `a`\" __ixTy.int a a\n"
    );
}

#[test]
fn destructured_annotation_must_be_inline_and_bound() {
    let error = diagnostic("type T = { a: int };\nexport default ({ a }: T) => a;\n");
    assert!(error.message().contains("inline object type"), "{error}");
    // Must name the other annotatable spelling too. An author reaching for the
    // cross product (destructured pattern, alias annotation) is reaching for
    // the shape that reads most naturally and does not exist, so being told
    // only that it is wrong leaves them guessing.
    assert!(error.message().contains("(params: Params)"), "{error}");
    let error = diagnostic("export default ({ a }: { b: int }) => a;\n");
    assert!(error.message().contains("not bound by the pattern"), "{error}");
}

#[test]
fn duplicate_object_type_fields_are_rejected() {
    // Two `a` fields have no sensible checker and no sensible schema, so the
    // declaration is rejected rather than resolved to one of them.
    let error = diagnostic("export default (x: { a: int; a: string }) => x;\n");
    assert!(error.message().contains("duplicate field `a`"), "{error}");
}

#[test]
fn number_is_rejected_naming_int_and_float() {
    let error = diagnostic("export default (x: number) => x;\n");
    assert!(error.message().contains("`int` or `float`"), "{error}");
}

#[test]
fn bool_and_boolean_both_lower() {
    // Found by the hegel reparse property: `bool` is a plain type reference
    // in TS (the keyword is `boolean`), so it needs its own builtin arm.
    for spelling in ["bool", "boolean"] {
        let out = nix(&format!("export default (x: {spelling}) => x;\n"));
        assert!(out.contains("__ixTy.bool"), "{spelling}: {out}");
    }
}

#[test]
fn lib_types_refinements_lower_to_runtime_checkers() {
    let out = nix("export default (p: port, d: path, s: nonEmptyStr, u: u32) => p;\n");
    for checker in ["__ixTy.port", "__ixTy.path", "__ixTy.nonEmptyStr", "__ixTy.u32"] {
        assert!(out.contains(checker), "{checker} missing in {out}");
    }
}

#[test]
fn destructured_optional_must_pair_with_a_nix_default() {
    // `{ a, b }` renders `{ a, b }:`, so Nix demands `b` whatever the
    // annotation says -- and the generated schema, reading only the `?`, would
    // have told a caller it was optional.
    let error = diagnostic("export default ({ a, b }: { a: int; b?: string }) => a;\n");
    assert!(error.message().contains("needs a default in the pattern"), "{error}");
    // The mirror spelling lies the other way: the default binds and then fails
    // the field's own check.
    let error = diagnostic("export default ({ a, b = null }: { a: int; b: string }) => a;\n");
    assert!(error.message().contains("type must be optional"), "{error}");
}

#[test]
fn an_alias_may_be_referenced_before_it_is_declared() {
    // Why `map::module` collects alias names in a pass of its own: an
    // annotation anywhere may name an alias declared later, and the emitted
    // `let` is recursive so order does not matter. Untested until now, and the
    // failure mode is a `ty'P` reference with no binding, i.e. an `undefined
    // variable` at eval rather than anything the converter would report.
    let out = nix("export default (x: P) => x;\ntype P = int;\n");
    assert!(out.contains("ty'P = __ixTy.int"), "{out}");
    assert!(out.contains("ty'P x"), "{out}");
}

#[test]
fn a_const_cannot_collide_with_an_alias_binding() {
    // `alias_binding` spells `ty'P` and claims a `const` can never collide,
    // because `'` is legal in a Nix identifier but not a JavaScript one. That
    // rests on the parser, so pin it: the collision is unspellable in source.
    let error = diagnostic("const ty'P = 1;\nexport default 1;\n");
    assert!(error.message().starts_with("parse error"), "{error}");
}

#[test]
fn an_unspellable_bound_field_name_is_rejected_at_the_pattern() {
    // The field check emits the bound name as a bare Nix identifier, so a
    // binder that is not one has to be refused. Both of these are legal
    // JavaScript identifiers and neither is a legal Nix one.
    for source in [
        "export default ({ then }: { then: int }) => 1;\n",
        "export default ({ $x }: { $x: int }) => 1;\n",
    ] {
        let error = diagnostic(source);
        assert!(
            error.message().contains("not a valid Nix identifier"),
            "{source}: {error}"
        );
        assert_eq!(error.column(), 19, "{source}: {error}");
    }
}

#[test]
fn readonly_property_signatures_are_rejected() {
    // No Nix meaning, and accepting it silently shifted a destructured field's
    // reported column from the key to the `readonly` keyword.
    let error = diagnostic("export default ({ a }: { readonly a: int }) => a;\n");
    assert!(error.message().contains("`readonly` has no Nix equivalent"), "{error}");
}

#[test]
fn float_literals_that_overflow_to_infinity_are_rejected() {
    // `inf.0` is not Nix syntax, and it serializes into a JSON Schema as
    // `null`. One check at the parse covers both positions.
    for source in ["export default 1e999;\n", "export default (x: 1e999 | 2) => x;\n"] {
        let error = diagnostic(source);
        assert!(error.message().contains("overflows to infinity"), "{source}: {error}");
    }
}

#[test]
fn optional_pattern_fields_check_as_nullable() {
    // The Nix default binds when the caller omits the field, so the bound
    // name's type is `T | null`, not `T`.
    let out = nix("export default ({ a = null }: { a?: int }) => a;\n");
    assert!(out.contains("(__ixTy.nullable __ixTy.int)"), "{out}");
}

#[test]
fn each_parameter_checks_inside_its_own_lambda() {
    // Checks fire on partial application and read exactly their own binder.
    let out = nix("export default (a: int, b: string) => a;\n");
    assert_eq!(
        out,
        "a: __ixTy.arg \"1:17 argument `a`\" __ixTy.int a (b: \
         __ixTy.arg \"1:25 argument `b`\" __ixTy.string b a)\n"
    );
}

#[test]
fn const_annotations_lower_to_ret_checks() {
    let out = nix("const x: int = 1;\nexport default x;\n");
    assert!(out.contains("x = __ixTy.ret \"1:8 const `x`\" __ixTy.int 1"), "{out}");
}

#[test]
fn builtin_shadowing_aliases_are_rejected() {
    let error = diagnostic("type port = string;\nexport default 1;\n");
    assert!(error.message().contains("shadows the built-in"), "{error}");
}

#[test]
fn optional_parameters_are_rejected_even_unannotated() {
    let error = diagnostic("export default (a?) => a;\n");
    assert!(error.message().contains("optional parameters"), "{error}");
}

#[test]
fn call_site_type_arguments_are_rejected() {
    let error = diagnostic("export default f<int>(2);\n");
    assert!(error.message().contains("call-site type arguments"), "{error}");
}

#[test]
fn definite_assignment_is_rejected() {
    let error = diagnostic("const x!: int = 1;\nexport default x;\n");
    assert!(error.message().contains("definite assignment"), "{error}");
}

#[test]
fn unknown_type_names_are_rejected() {
    let error = diagnostic("export default (x: Widget) => x;\n");
    assert!(error.message().contains("unknown type `Widget`"), "{error}");
}

#[test]
fn mixed_unions_are_rejected() {
    let error = diagnostic("export default (x: int | string) => x;\n");
    assert!(error.message().contains("`T | null`"), "{error}");
}

#[test]
fn generics_interfaces_satisfies_and_nonnull_are_rejected() {
    let error = diagnostic("export default <A>(x: A) => x;\n");
    assert!(error.message().contains("generic arrows"), "{error}");
    let error = diagnostic("type Box<A> = { v: A };\nexport default 1;\n");
    assert!(error.message().contains("generic type aliases"), "{error}");
    let error = diagnostic("interface I { a: int }\nexport default 1;\n");
    assert!(error.message().contains("use `type`"), "{error}");
    let error = diagnostic("export default x satisfies int;\n");
    assert!(error.message().contains("static-only"), "{error}");
    let error = diagnostic("export default x!;\n");
    assert!(error.message().contains("no runtime lowering"), "{error}");
}

#[test]
fn duplicate_type_aliases_are_rejected() {
    let error = diagnostic("type A = int;\ntype A = float;\nexport default 1;\n");
    assert!(error.message().contains("duplicate `type A`"), "{error}");
}

#[test]
fn unannotated_modules_lower_without_any_ixty_reference() {
    let out = nix("const f = (a) => a;\nexport default f(1);\n");
    assert!(!out.contains("__ixTy"), "{out}");
}

// --- module shape ---

#[test]
fn module_without_export_default_is_rejected() {
    let error = diagnostic("const a = 1;\n");
    assert!(error.message().contains("export default"), "{error}");
}

#[test]
fn non_const_top_level_statement_is_rejected() {
    let error = diagnostic("console.log(1);\nexport default 1;\n");
    assert!(error.message().contains("top level"), "{error}");
    assert_eq!((error.line(), error.column()), (1, 1));
}

#[test]
fn top_level_consts_become_a_let() {
    assert_eq!(nix("const a = 1;\nexport default a;\n"), "let\n  a = 1;\nin\na\n");
}

#[test]
fn wrapper_appears_even_without_imports() {
    // `nix` strips the wrapper (asserting it), so an import-free module
    // reduces to its bare body.
    assert_eq!(nix("export default 1;\n"), "1\n");
}

#[test]
fn let_and_var_are_rejected() {
    let error = diagnostic("let a = 1;\nexport default a;\n");
    assert!(error.message().contains("`const`"), "{error}");
    let error = diagnostic("var a = 1;\nexport default a;\n");
    assert!(error.message().contains("`const`"), "{error}");
}

#[test]
fn duplicate_const_names_are_rejected() {
    // One `const a` per declaration: redeclaration across statements is valid
    // syntax to the parser but has no Nix `let` spelling.
    let error = diagnostic("const a = 1;\nconst a = 2;\nexport default a;\n");
    assert!(error.message().contains("duplicate `const a`"), "{error}");
}

#[test]
fn destructuring_const_is_rejected() {
    let error = diagnostic("const { a } = b;\nexport default a;\n");
    assert!(error.message().contains("destructuring"), "{error}");
}

// --- literals and identifiers ---

#[test]
fn numeric_notations_normalize_to_nix_integers() {
    assert_eq!(nix("export default 1_000_000;"), "1000000\n");
    assert_eq!(nix("export default 0xff;"), "255\n");
    assert_eq!(nix("export default 0b101;"), "5\n");
    assert_eq!(nix("export default 0o755;"), "493\n");
}

#[test]
fn floats_stay_floats() {
    assert_eq!(nix("export default 1.5;"), "1.5\n");
    assert_eq!(nix("export default 2e3;"), "2000.0\n");
}

#[test]
fn oversized_integer_is_rejected() {
    let error = diagnostic("export default 99999999999999999999;");
    assert!(error.message().contains("64-bit"), "{error}");
}

#[test]
fn undefined_is_rejected_with_null_fix() {
    let error = diagnostic("export default undefined;");
    assert!(error.message().contains("use `null`"), "{error}");
}

#[test]
fn nix_keyword_identifiers_are_rejected() {
    let error = diagnostic("export default then;");
    assert!(error.message().contains("not a valid Nix identifier"), "{error}");
    let error = diagnostic("export default $money;");
    assert!(error.message().contains("not a valid Nix identifier"), "{error}");
}

// --- functions ---

#[test]
fn zero_parameter_arrow_is_rejected() {
    let error = diagnostic("export default () => 1;");
    assert!(error.message().contains("exactly one argument"), "{error}");
}

#[test]
fn plain_parameter_default_is_rejected() {
    let error = diagnostic("export default (a = 1) => a;");
    assert!(error.message().contains("destructured object"), "{error}");
}

#[test]
fn pattern_renaming_is_rejected() {
    let error = diagnostic("export default ({ a: b }) => b;");
    assert!(error.message().contains("renaming"), "{error}");
}

#[test]
fn closed_pattern_has_no_ellipsis() {
    assert_eq!(nix("export default ({ a }) => a;"), "{ a }: a\n");
}

#[test]
fn arrow_block_allows_only_consts_then_return() {
    let error = diagnostic("export default (a) => { a += 1; return a; };");
    assert!(error.message().contains("let ... in"), "{error}");
    let error = diagnostic("export default (a) => { for (;;) {} return a; };");
    assert!(error.message().contains("let ... in"), "{error}");
    let error = diagnostic("export default (a) => { if (a) { return a; } return a; };");
    assert!(error.message().contains("let ... in"), "{error}");
}

#[test]
fn arrow_block_without_return_is_rejected() {
    let error = diagnostic("export default (a) => { const b = a; };");
    assert!(error.message().contains("return"), "{error}");
}

#[test]
fn statement_after_return_is_rejected() {
    let error = diagnostic("export default (a) => { return a; const b = 1; };");
    assert!(error.message().contains("unreachable"), "{error}");
}

#[test]
fn function_expression_is_rejected_with_arrow_fix() {
    let error = diagnostic("export default function (a) { return a; };");
    assert!(error.message().contains("arrow function"), "{error}");
}

// --- calls ---

#[test]
fn zero_argument_call_is_rejected() {
    let error = diagnostic("export default f();");
    assert!(error.message().contains("zero-argument"), "{error}");
}

#[test]
fn spread_argument_is_rejected() {
    let error = diagnostic("export default f(...a);");
    assert!(error.message().contains("spread"), "{error}");
}

// --- objects, arrays, selection ---

#[test]
fn spread_only_object_is_the_operand_itself() {
    assert_eq!(nix("export default { ...a };"), "a\n");
    assert_eq!(nix("export default {};"), "{}\n");
}

#[test]
fn duplicate_object_keys_are_rejected() {
    let error = diagnostic("export default { a: 1, a: 2 };");
    assert!(error.message().contains("duplicate key `a`"), "{error}");
    // Shorthand names the key too, so it collides with a written-out one.
    let error = diagnostic("export default { a, a: 2 };");
    assert!(error.message().contains("duplicate key `a`"), "{error}");
}

#[test]
fn shorthand_property_repeats_the_name_on_both_sides() {
    // `{ src }` is the JS spelling of `src = src;`. Pattern position already
    // requires the shorthand (renaming is rejected there), so pin the literal
    // position too: entrypoints thread flake values through it.
    assert_eq!(nix("export default { src };"), "{\n  src = src;\n}\n");
    assert_eq!(
        nix("export default { src, mode: 1 };"),
        "{\n  src = src;\n  mode = 1;\n}\n"
    );
}

#[test]
fn getters_and_methods_are_rejected() {
    let error = diagnostic("export default { get a() { return 1; } };");
    assert!(error.message().contains("getters"), "{error}");
    let error = diagnostic("export default { a() { return 1; } };");
    assert!(error.message().contains("methods"), "{error}");
}

#[test]
fn array_holes_are_rejected() {
    let error = diagnostic("export default [1, , 2];");
    assert!(error.message().contains("holes"), "{error}");
}

#[test]
fn numeric_indexing_is_rejected_with_elem_at_fix() {
    let error = diagnostic("export default xs[0];");
    assert!(error.message().contains("builtins.elemAt"), "{error}");
}

#[test]
fn bare_coalesce_is_rejected() {
    let error = diagnostic("export default a ?? b;");
    assert!(error.message().contains("optional chain"), "{error}");
}

#[test]
fn bare_optional_chain_is_rejected() {
    let error = diagnostic("export default a?.b;");
    assert!(error.message().contains("??"), "{error}");
}

#[test]
fn optional_chain_with_default_becomes_or() {
    assert_eq!(nix("export default a.b?.c ?? 1;"), "a.b.c or 1\n");
}

// --- operators ---

#[test]
fn strict_equality_is_rejected_naming_the_fix() {
    let error = diagnostic("export default a === b;");
    assert!(error.message().contains("use `==`"), "{error}");
    let error = diagnostic("export default a !== b;");
    assert!(error.message().contains("use `!=`"), "{error}");
}

#[test]
fn operators_without_nix_equivalents_are_rejected() {
    for source in [
        "export default a % b;",
        "export default a ** b;",
        "export default a & b;",
        "export default a instanceof b;",
        "export default typeof a;",
        "export default a++;",
    ] {
        assert!(convert(source).is_err(), "{source} should be rejected");
    }
}

// --- imports ---

#[test]
fn import_specifiers_must_be_relative_literal_module_paths() {
    let error = diagnostic("export default import(\"lodash\");");
    assert!(error.message().contains("relative"), "{error}");
    let error = diagnostic("export default import(\"./x.js\");");
    assert!(error.message().contains(".ix"), "{error}");
    let error = diagnostic("export default import(name);");
    assert!(error.message().contains("string literal"), "{error}");
}

// --- diagnostics ---

#[test]
fn parse_errors_are_positioned() {
    let error = diagnostic("export default (;\n");
    assert!(error.message().starts_with("parse error"), "{error}");
    assert_eq!(error.line(), 1);
}

#[test]
fn errors_render_like_compiler_diagnostics() {
    let error = diagnostic("export default a === b;\n");
    let rendered = error.to_string();
    assert!(rendered.starts_with("error: "), "{rendered}");
    assert!(rendered.contains("--> 1:16"), "{rendered}");
    assert!(rendered.contains("export default a === b;"), "{rendered}");
}
