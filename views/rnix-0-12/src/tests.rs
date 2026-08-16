use std::{ffi::OsStr, fmt::Write, fs, path::PathBuf};

use expect_test::expect_file;
use rowan::ast::AstNode;

use crate::{
    ast::{self, HasEntry},
    tokenize, Root, SyntaxKind,
};

#[test]
fn interpolation() {
    let root = ast::Root::parse(include_str!("../test_data/parser/success/interpolation.nix"))
        .ok()
        .unwrap();
    let let_in = ast::LetIn::try_from(root.expr().unwrap()).unwrap();
    let set = ast::AttrSet::try_from(let_in.body().unwrap()).unwrap();
    let entry = set.entries().nth(1).unwrap();
    let attrpath_value = ast::AttrpathValue::try_from(entry).unwrap();
    let value = ast::Str::try_from(attrpath_value.value().unwrap()).unwrap();

    match &*value.normalized_parts() {
    &[
        ast::InterpolPart::Literal(ref s1),
        ast::InterpolPart::Interpolation(_),
        ast::InterpolPart::Literal(ref s2),
        ast::InterpolPart::Interpolation(_),
        ast::InterpolPart::Literal(ref s3)
    ]
    if s1 == "The set's x value is: "
        && s2 == "\n\nThis line shall have no indention\n  This line shall be indented by 2\n\n\n"
        && s3 == "\n" => (),
    parts => panic!("did not match: {:#?}", parts)
}
}

#[test]
fn inherit() {
    let root =
        ast::Root::parse(include_str!("../test_data/parser/success/inherit.nix")).ok().unwrap();
    let let_in = ast::LetIn::try_from(root.expr().unwrap()).unwrap();
    let set = ast::AttrSet::try_from(let_in.body().unwrap()).unwrap();
    let inherit = set.inherits().nth(1).unwrap();

    let from = inherit.from().unwrap().expr().unwrap();
    let ident: ast::Ident = ast::Ident::try_from(from).unwrap();
    assert_eq!(ident.syntax().text(), "set");
    let mut children = inherit.attrs();
    assert_eq!(children.next().unwrap().syntax().text(), "z");
    assert_eq!(children.next().unwrap().syntax().text(), "a");
    assert!(children.next().is_none());
}

#[test]
fn math() {
    let root = ast::Root::parse(include_str!("../test_data/parser/success/math.nix")).ok().unwrap();
    let op1 = ast::BinOp::try_from(root.expr().unwrap()).unwrap();
    let op2 = ast::BinOp::try_from(op1.lhs().unwrap()).unwrap();
    assert_eq!(op1.operator().unwrap(), ast::BinOpKind::Add);

    let lhs = ast::Literal::try_from(op2.lhs().unwrap()).unwrap();
    assert_eq!(lhs.syntax().text(), "1");

    let rhs = ast::BinOp::try_from(op2.rhs().unwrap()).unwrap();
    assert_eq!(rhs.operator().unwrap(), ast::BinOpKind::Mul);
}

#[test]
fn t_macro() {
    assert_eq!(T![@], SyntaxKind::TOKEN_AT);
    assert!(matches!(SyntaxKind::TOKEN_L_PAREN, T!['(']));
}

fn dir_tests<F>(dir: &str, get_actual: F)
where
    F: Fn(String) -> String,
{
    let base_path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "test_data", dir].iter().collect();
    let success_path = base_path.join("success");
    let error_path = base_path.join("error");

    let entries = success_path.read_dir().unwrap().chain(error_path.read_dir().unwrap());

    for entry in entries {
        let path = entry.unwrap().path();

        if path.extension() != Some(OsStr::new("nix")) {
            continue;
        }

        println!("testing: {}", path.display());

        let mut code = fs::read_to_string(&path).unwrap();
        if code.ends_with('\n') {
            code.truncate(code.len() - 1);
        }

        let actual = get_actual(code);
        expect_file![path.with_extension("expect")].assert_eq(&actual);
    }
}

/// A separator the tokenizer admitted has to disappear before the value is
/// parsed: `str::parse` rejects `_`, so a tokenizer that accepts `1_000`
/// without this returns `Err` from `value()` and the literal is lost in a
/// different place rather than lexed correctly.
///
/// The tokenizer fixtures cover which spans become one numeric token; only
/// this covers what number that token is worth.
#[test]
fn numeric_separators_do_not_change_the_value() {
    fn int(code: &str) -> i64 {
        let root = ast::Root::parse(code).ok().unwrap();
        match ast::Literal::try_from(root.expr().unwrap()).unwrap().kind() {
            ast::LiteralKind::Integer(i) => i.value().unwrap(),
            other => panic!("{code}: not an integer literal: {other:?}"),
        }
    }
    fn float(code: &str) -> f64 {
        let root = ast::Root::parse(code).ok().unwrap();
        match ast::Literal::try_from(root.expr().unwrap()).unwrap().kind() {
            ast::LiteralKind::Float(f) => f.value().unwrap(),
            other => panic!("{code}: not a float literal: {other:?}"),
        }
    }

    assert_eq!(int("1_000"), 1000);
    assert_eq!(int("1_000_000"), 1000000);
    assert_eq!(int("1_0_0"), 100);
    assert_eq!(int("1__0"), 10);
    assert_eq!(int("9_223_372_036_854_775_806"), 9223372036854775806);

    assert_eq!(float("1_000.000_1"), 1000.0001);
    assert_eq!(float("0.000_100"), 0.0001);
    assert_eq!(float(".5_0"), 0.50);
    assert_eq!(float("2.5e1_0"), 2.5e10);
    assert_eq!(float("6.674_30e-1_1"), 6.67430e-11);

    // A literal with no separator still reads the same, and takes the
    // borrowing path through `without_separators`.
    assert_eq!(int("1000"), 1000);
    assert_eq!(float("1.234"), 1.234);
}

#[test]
fn parser_dir_tests() {
    dir_tests("parser", |code| {
        let parse = Root::parse(&code);

        let mut actual = String::new();
        for error in parse.errors() {
            writeln!(actual, "error: {}", error).unwrap();
        }
        writeln!(actual, "{:#?}", parse.syntax()).unwrap();

        actual
    })
}

#[test]
fn tokenizer_dir_tests() {
    dir_tests("tokenizer", |code| {
        let mut actual = String::new();
        for (kind, str) in tokenize(&code) {
            writeln!(actual, "{:?}, \"{}\"", kind, str).unwrap();
        }
        actual
    })
}
