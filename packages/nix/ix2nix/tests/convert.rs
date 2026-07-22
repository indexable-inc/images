//! Behavior tests across the public boundary: `.ix` source in, Nix source or
//! a positioned diagnostic out.

use ix2nix::convert;

fn nix(source: &str) -> String {
    convert(source).expect("source should convert")
}

fn diagnostic(source: &str) -> ix2nix::Error {
    convert(source).expect_err("source should be rejected")
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
fn wrapper_appears_exactly_when_import_is_used() {
    assert!(nix("export default import(\"./x.ix\");\n").starts_with("{ __dir, __importIx }:\n"));
    assert!(!nix("export default 1;\n").starts_with("{ __dir"));
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
