use ast_merge_ast::tree;
use ast_merge_langs::Lang;

use crate::{Unit, measure};

fn units(lang: Lang, source: &str) -> Vec<Unit> {
    let parsed = tree(source, &lang.to_tree_sitter()).expect("sample parses");
    assert!(!parsed.has_errors, "{lang:?} sample has parse errors");
    measure(&parsed.tree, lang)
}

fn only(lang: Lang, source: &str) -> Unit {
    let mut found = units(lang, source);
    assert_eq!(found.len(), 1, "expected exactly one unit: {found:?}");
    found.remove(0)
}

/// One scoring rule, its minimal example, and the three numbers it pins.
struct Case {
    /// Names the rule under test, so a failure says which one broke.
    rule: &'static str,
    lang: Lang,
    source: &'static str,
    cognitive: u32,
    cyclomatic: u32,
    nesting: u32,
}

/// The rules that differ only in their example and their expected numbers.
/// Held as a table rather than one function each: the whole point of the
/// metric is how these cases compare, and a table shows that where a run of
/// near-identical test functions hides it.
const CASES: &[Case] = &[
    // `else if` increments but carries no nesting penalty, so a three-arm
    // chain costs 3 rather than 1 + 2 + 3.
    Case {
        rule: "else if carries no nesting penalty",
        lang: Lang::Rust,
        source: r"
fn f(a: u32) -> u32 {
    if a == 0 { 1 } else if a == 1 { 2 } else { 3 }
}
",
        cognitive: 3,
        cyclomatic: 3,
        nesting: 1,
    },
    // Go spells `else if` as a bare `if_statement` in the parent's
    // `alternative` field rather than wrapping it, so the demotion has to key
    // on the field and not on a wrapper kind.
    Case {
        rule: "go else if is demoted through the alternative field",
        lang: Lang::Go,
        source: "
func f(a int) int {
\tif a == 0 {
\t\treturn 1
\t} else if a == 1 {
\t\treturn 2
\t}
\treturn 3
}
",
        cognitive: 2,
        cyclomatic: 3,
        nesting: 1,
    },
    // A match is one cognitive increment however many arms it has, while each
    // arm is its own independent path. The clearest case where the two metrics
    // disagree on purpose.
    Case {
        rule: "match costs once cognitively and per arm cyclomatically",
        lang: Lang::Rust,
        source: r"
fn f(a: u32) -> u32 {
    match a { 0 => 1, 1 => 2, 2 => 3, _ => 4 }
}
",
        cognitive: 1,
        cyclomatic: 5,
        nesting: 1,
    },
    // The same disagreement in a language that spells it `switch`.
    Case {
        rule: "typescript switch costs once cognitively",
        lang: Lang::TypeScript,
        source: r"
function f(a: number): number {
  switch (a) {
    case 1: return 1;
    case 2: return 2;
    default: return 3;
  }
}
",
        cognitive: 1,
        cyclomatic: 3,
        nesting: 1,
    },
    // Elixir has no keyword node kinds: `def`, `case` and `if` are calls, and
    // the dispatch arms are `stab_clause`s.
    Case {
        rule: "elixir units are calls and arms are stab clauses",
        lang: Lang::Elixir,
        source: r"
defmodule M do
  def f(a) do
    case a do
      1 -> :one
      _ -> :other
    end
  end
end
",
        cognitive: 1,
        cyclomatic: 3,
        nesting: 1,
    },
    // Python charges `elif` and `else` flat, and folds the `and` run to one.
    Case {
        rule: "python elif and boolean runs",
        lang: Lang::Python,
        source: r"
def f(a, b):
    if a and b and a:
        return 1
    elif a:
        return 2
    else:
        return 3
",
        cognitive: 4,
        cyclomatic: 5,
        nesting: 1,
    },
];

#[test]
fn scoring_rules() {
    for case in CASES {
        let unit = only(case.lang, case.source);
        assert_eq!(unit.cognitive, case.cognitive, "cognitive: {}", case.rule);
        assert_eq!(
            unit.cyclomatic, case.cyclomatic,
            "cyclomatic: {}",
            case.rule
        );
        assert_eq!(unit.nesting, case.nesting, "nesting: {}", case.rule);
    }
}

/// The property the whole metric exists for, and the one case that needs two
/// samples side by side: three flat branches and three nested ones are the
/// same cyclomatic number and very different cognitive ones. The nested body
/// is Campbell's own worked example (white paper, section 3) transliterated to
/// Rust; the labelled jump he also charges is not scored here, so cognitive
/// comes out one below his 7.
#[test]
fn nesting_costs_more_than_breadth() {
    let nested = only(
        Lang::Rust,
        r"
fn sum_of_primes(max: u32) -> u32 {
    let mut total = 0;
    for i in 1..=max {
        for j in 2..i {
            if i % j == 0 {
                continue;
            }
        }
        total += i;
    }
    total
}
",
    );

    let flat = only(
        Lang::Rust,
        r"
fn flat(a: u32, b: u32, c: u32) -> u32 {
    if a == 0 { return 1; }
    if b == 0 { return 2; }
    if c == 0 { return 3; }
    0
}
",
    );

    assert_eq!(nested.cyclomatic, 4);
    assert_eq!(nested.cognitive, 6);
    assert_eq!(nested.nesting, 3);

    assert_eq!(flat.cyclomatic, 4);
    assert_eq!(flat.cognitive, 3);
    assert_eq!(flat.nesting, 1);
}

/// A run of like operators is one cognitive increment; a change of operator
/// starts a new run (Campbell, appendix B1). Cyclomatic does not fold runs.
#[test]
fn logical_runs_count_once_per_run() {
    let one_run = only(
        Lang::Rust,
        "fn f(a: bool, b: bool, c: bool) -> bool { a && b && c }",
    );
    let two_runs = only(
        Lang::Rust,
        "fn f(a: bool, b: bool, c: bool) -> bool { a && b || c }",
    );

    assert_eq!(one_run.cognitive, 1);
    assert_eq!(two_runs.cognitive, 2);

    assert_eq!(one_run.cyclomatic, 3);
    assert_eq!(two_runs.cyclomatic, 3);
}

/// A closure deepens its body without scoring for itself, and it is absorbed
/// into the enclosing function rather than reported separately.
#[test]
fn closures_deepen_without_being_reported() {
    let found = units(
        Lang::Rust,
        r"
fn f(xs: &[u32]) -> Vec<u32> {
    xs.iter().map(|x| if *x > 0 { 1 } else { 0 }).collect()
}
",
    );
    assert_eq!(found.len(), 1);
    let unit = &found[0];
    // The `if` sits at nesting 1 inside the closure, so 2, plus 1 for the else.
    assert_eq!(unit.cognitive, 3);
    assert_eq!(unit.nesting, 2);
}

/// Nix reports bindings. The lambda chain carrying a binding's parameters is
/// the unit's own head, so it must not deepen: without that rule every `if` in
/// every Nix function would start at a nesting penalty it has not earned.
#[test]
fn nix_reports_bindings_and_does_not_charge_for_its_own_parameters() {
    let found = units(
        Lang::Nix,
        r"
{
  f = a: if a then with a; 1 else 2;
}
",
    );
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].cognitive, 1);
    assert!(found[0].signature.starts_with("f ="));
}

/// A Nix binding whose value is an attribute set is a namespace: its members
/// are the units, and reporting the wrapper instead would collapse a whole
/// file into one entry carrying everyone else's score.
#[test]
fn nix_attrset_bindings_are_namespaces_not_units() {
    let found = units(
        Lang::Nix,
        r"
{
  group = {
    a = x: if x then 1 else 2;
    b = y: if y then 3 else 4;
  };
}
",
    );
    let signatures: Vec<&str> = found.iter().map(|unit| unit.signature.as_str()).collect();
    assert_eq!(
        signatures,
        vec!["a = x: if x then 1 else 2;", "b = y: if y then 3 else 4;"]
    );
    assert!(found.iter().all(|unit| unit.cognitive == 1));
}

/// A language with no profile contributes nothing rather than contributing
/// zeros, so it cannot dilute a repo-wide budget.
#[test]
fn unprofiled_languages_yield_no_units() {
    assert!(units(Lang::Json, r#"{"a": 1}"#).is_empty());
}
