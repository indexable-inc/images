//! Per-language classification of tree-sitter node kinds.
//!
//! Every kind here was transcribed from the output of `src/dump.rs`, which
//! parses a sample exercising each construct and prints the kinds the grammar
//! actually produced. A misspelled kind scores zero silently rather than
//! failing, so guessing them is not safe.

use ast_merge_langs::Lang;

/// How a node kind affects the two scores.
///
/// The three cognitive categories are Campbell's B1/B2/B3 split: `nesting` is
/// a structure that both increments and deepens (`if`, a loop, `catch`),
/// `flat` increments without a nesting penalty (`else`, where the reader has
/// already paid the cost of the `if`), and `structural` deepens without
/// incrementing (a closure body).
pub struct Profile {
    /// Kinds that open a reported unit. A unit nested inside another unit is
    /// absorbed into it rather than reported separately, matching `SonarQube`.
    pub units: &'static [&'static str],
    /// Elixir has no `def` node kind: a definition is a `call` whose target
    /// identifier is `def`. Empty for every grammar with real function nodes.
    pub unit_calls: &'static [&'static str],
    /// Cyclomatic: each occurrence is one more independent path.
    pub decisions: &'static [&'static str],
    /// Cyclomatic, by call target, for the same reason as `unit_calls`.
    pub decision_calls: &'static [&'static str],
    /// Cognitive: +1, plus the current nesting level, and deepens its body.
    pub nesting: &'static [&'static str],
    /// Cognitive nesting structures addressed by call target.
    pub nesting_calls: &'static [&'static str],
    /// Cognitive: +1 with no nesting penalty and no deepening.
    pub flat: &'static [&'static str],
    /// Cognitive: deepens its body without incrementing.
    pub structural: &'static [&'static str],
    /// Value kinds that make a unit candidate a namespace rather than a unit.
    /// A Nix binding whose value is an attribute set groups other bindings; it
    /// is not itself something anyone reads top to bottom, and reporting it
    /// would absorb every member into one entry thousands of lines long.
    pub namespace_values: &'static [&'static str],
    /// Kinds carrying a binary operator. A run of like operators counts once
    /// cognitively, so `a && b && c` is +1 and `a && b || c` is +2.
    pub logical: &'static [&'static str],
    /// Operator spellings treated as logical for the run rule.
    pub logical_ops: &'static [&'static str],
}

const NONE: &[&str] = &[];
const SYMBOLIC_OPS: &[&str] = &["&&", "||"];

const RUST: Profile = Profile {
    units: &["function_item"],
    unit_calls: NONE,
    // `loop_expression` has no condition, but it is exited by `break`, so the
    // CFG it produces still carries an extra independent path.
    // `?` is deliberately absent: it is an early return, but it is so dense in
    // this repo that counting it would make the metric a count of fallible
    // calls rather than of branching.
    decisions: &[
        "if_expression",
        "while_expression",
        "for_expression",
        "loop_expression",
        "match_arm",
    ],
    decision_calls: NONE,
    nesting: &[
        "if_expression",
        "while_expression",
        "for_expression",
        "loop_expression",
        "match_expression",
    ],
    nesting_calls: NONE,
    flat: &["else_clause"],
    structural: &["closure_expression"],
    namespace_values: NONE,
    logical: &["binary_expression"],
    logical_ops: SYMBOLIC_OPS,
};

const PYTHON: Profile = Profile {
    units: &["function_definition"],
    unit_calls: NONE,
    decisions: &[
        "if_statement",
        "elif_clause",
        "while_statement",
        "for_statement",
        "except_clause",
        "conditional_expression",
        "case_clause",
    ],
    decision_calls: NONE,
    nesting: &[
        "if_statement",
        "while_statement",
        "for_statement",
        "except_clause",
        "conditional_expression",
        "match_statement",
    ],
    nesting_calls: NONE,
    flat: &["elif_clause", "else_clause"],
    structural: &["lambda"],
    namespace_values: NONE,
    logical: &["boolean_operator"],
    logical_ops: &["and", "or"],
};

const TYPESCRIPT: Profile = Profile {
    units: &[
        "function_declaration",
        "generator_function_declaration",
        "method_definition",
    ],
    unit_calls: NONE,
    // `switch_default` is not a decision: it is the fall-through, not a branch.
    decisions: &[
        "if_statement",
        "while_statement",
        "do_statement",
        "for_statement",
        "for_in_statement",
        "catch_clause",
        "ternary_expression",
        "switch_case",
    ],
    decision_calls: NONE,
    nesting: &[
        "if_statement",
        "while_statement",
        "do_statement",
        "for_statement",
        "for_in_statement",
        "catch_clause",
        "ternary_expression",
        "switch_statement",
    ],
    nesting_calls: NONE,
    flat: &["else_clause"],
    structural: &["arrow_function", "function_expression"],
    namespace_values: NONE,
    logical: &["binary_expression"],
    logical_ops: SYMBOLIC_OPS,
};

const GO: Profile = Profile {
    units: &["function_declaration", "method_declaration"],
    unit_calls: NONE,
    decisions: &[
        "if_statement",
        "for_statement",
        "expression_case",
        "type_case",
        "communication_case",
    ],
    decision_calls: NONE,
    nesting: &[
        "if_statement",
        "for_statement",
        "expression_switch_statement",
        "type_switch_statement",
        "select_statement",
    ],
    nesting_calls: NONE,
    // Go spells `else if` as an `if_statement` in the parent's `alternative`
    // field; `demoted_to_flat` handles it, so there is no separate else kind.
    flat: NONE,
    structural: &["func_literal"],
    namespace_values: NONE,
    logical: &["binary_expression"],
    logical_ops: SYMBOLIC_OPS,
};

const NIX: Profile = Profile {
    // Nix has no statements and no functions in the usual sense: the unit a
    // reader navigates by is the attribute binding, so that is what is
    // reported. A binding nested in a `let` inside another binding is absorbed
    // into the outer one.
    units: &["binding"],
    unit_calls: NONE,
    decisions: &["if_expression"],
    decision_calls: NONE,
    nesting: &["if_expression"],
    nesting_calls: NONE,
    flat: NONE,
    // `with` opens a scope whose names cannot be traced to a definition, which
    // is why the repo's astlog rules ban it outright; charging it as a nesting
    // level says the same thing in a number.
    structural: &["function_expression", "with_expression"],
    namespace_values: &["attrset_expression", "rec_attrset_expression"],
    logical: &["binary_expression"],
    logical_ops: SYMBOLIC_OPS,
};

const ELIXIR: Profile = Profile {
    units: NONE,
    unit_calls: &["def", "defp", "defmacro", "defmacrop"],
    // Each `->` clause is one dispatch arm, whether it belongs to a `case`, a
    // `cond`, or a multi-clause anonymous function.
    decisions: &["stab_clause"],
    decision_calls: &["if", "unless"],
    nesting: NONE,
    nesting_calls: &[
        "if", "unless", "case", "cond", "receive", "try", "for", "with",
    ],
    flat: NONE,
    structural: &["anonymous_function"],
    namespace_values: NONE,
    logical: &["binary_operator"],
    logical_ops: &["and", "or", "&&", "||"],
};

/// The classification for a language, or `None` when the language is not yet
/// covered. An uncovered language is skipped rather than scored zero, so it
/// cannot silently dilute a repo-wide budget.
#[must_use]
pub const fn profile(lang: Lang) -> Option<&'static Profile> {
    match lang {
        Lang::Rust => Some(&RUST),
        Lang::Python => Some(&PYTHON),
        Lang::TypeScript | Lang::TypeScriptTsx | Lang::JavaScript => Some(&TYPESCRIPT),
        Lang::Go => Some(&GO),
        Lang::Nix => Some(&NIX),
        Lang::Elixir => Some(&ELIXIR),
        _ => None,
    }
}
