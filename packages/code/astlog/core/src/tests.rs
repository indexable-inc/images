use crate::{Severity, Value, analyze};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const RUST_SAMPLE: &str = r"
fn fallible() -> Result<u32, String> {
    let n = compute().unwrap();
    Ok(n + 1)
}

fn infallible() -> u32 {
    compute().unwrap()
}

fn compute() -> Result<u32, String> {
    Ok(41)
}
";

const UNWRAP_RULES: &str = r#"
; `<e>.unwrap()` call sites
(rule (unwrap-call call e)
  (match rust "
    (call_expression
      function: (field_expression value: (_) @e field: (field_identifier) @m)
      arguments: (arguments)) @call")
  (text m "unwrap"))

; functions whose return type is Result<...>
(rule (result-fn f)
  (match rust "
    (function_item return_type: (generic_type type: (type_identifier) @r)) @f")
  (text r "Result"))

; the join: an unwrap call lexically inside a Result-returning function
(rule (fixable call e)
  (unwrap-call call e)
  (result-fn f)
  (ancestor f call))

(rewrite unwrap-to-try (fixable call e)
  (replace call "{e}?"))
"#;

fn write_sample(dir: &tempfile::TempDir, name: &str, content: &str) -> TestResult {
    std::fs::write(dir.path().join(name), content)?;
    Ok(())
}

#[test]
fn unwrap_join_finds_only_result_functions() -> TestResult {
    let dir = tempfile::tempdir()?;
    write_sample(&dir, "sample.rs", RUST_SAMPLE)?;
    let analysis = analyze(UNWRAP_RULES, &[dir.path().to_path_buf()])?;

    let unwrap_calls = &analysis.database.relations["unwrap-call"];
    assert_eq!(unwrap_calls.rows().len(), 2, "both unwrap sites match");

    let fixable = &analysis.database.relations["fixable"];
    assert_eq!(fixable.rows().len(), 1, "only the Result-returning fn joins");
    let Value::Node(call) = &fixable.rows()[0][0] else {
        return Err("fixable column 0 should be a node".into());
    };
    assert_eq!(analysis.corpus.node_text(*call), "compute().unwrap()");
    Ok(())
}

#[test]
fn rewrite_splices_template() -> TestResult {
    let dir = tempfile::tempdir()?;
    write_sample(&dir, "sample.rs", RUST_SAMPLE)?;
    let analysis = analyze(UNWRAP_RULES, &[dir.path().to_path_buf()])?;

    assert_eq!(analysis.edits.len(), 1);
    let rewritten = analysis.rewritten();
    assert_eq!(rewritten.len(), 1);
    assert!(rewritten[0].content.contains("let n = compute()?;"));
    assert!(
        rewritten[0].content.contains("compute().unwrap()"),
        "the infallible fn keeps its unwrap"
    );
    let diff = analysis.diff();
    assert!(diff.contains("-    let n = compute().unwrap();"));
    assert!(diff.contains("+    let n = compute()?;"));
    Ok(())
}

#[test]
fn value_join_on_same_text() -> TestResult {
    let source = r#"
fn main() {
    let secret = std::env::var("KEY");
    let safe = "literal";
    run(secret);
    run(safe);
    run(other);
}
"#;
    // join let-bound names against call arguments on identifier text
    let rules = r#"
(rule (env-bound v)
  (match rust "
    (let_declaration
      pattern: (identifier) @v
      value: (call_expression function: (scoped_identifier name: (identifier) @f)))")
  (text f "var"))

(rule (call-arg call v)
  (match rust "(call_expression arguments: (arguments (identifier) @v)) @call"))

(rule (tainted-call call)
  (call-arg call v)
  (env-bound w)
  (same-text v w))
"#;
    let dir = tempfile::tempdir()?;
    write_sample(&dir, "sample.rs", source)?;
    let analysis = analyze(rules, &[dir.path().to_path_buf()])?;

    let tainted = &analysis.database.relations["tainted-call"];
    assert_eq!(tainted.rows().len(), 1, "only run(secret) joins");
    let Value::Node(call) = &tainted.rows()[0][0] else {
        return Err("tainted-call column 0 should be a node".into());
    };
    assert_eq!(analysis.corpus.node_text(*call), "run(secret)");
    Ok(())
}

#[test]
fn recursive_rules_reach_fixpoint() -> TestResult {
    let source = "fn main() { if true { loop { break; } } }\n";
    // `up` is recursive: parent plus transitive step. It must agree with the
    // ancestor builtin once both are restricted to the same endpoints.
    let rules = r#"
(rule (brk b) (match rust "(break_expression) @b"))
(rule (fun f) (match rust "(function_item) @f"))

(rule (up x y) (brk y) (parent x y))
(rule (up x z) (up y z) (parent x y))

(rule (reaches f b) (fun f) (brk b) (up f b))
(rule (reaches-builtin f b) (fun f) (brk b) (ancestor f b))
"#;
    let dir = tempfile::tempdir()?;
    write_sample(&dir, "sample.rs", source)?;
    let analysis = analyze(rules, &[dir.path().to_path_buf()])?;

    let recursive = &analysis.database.relations["reaches"];
    let builtin = &analysis.database.relations["reaches-builtin"];
    assert_eq!(recursive.rows().len(), 1);
    assert_eq!(recursive.rows(), builtin.rows());
    Ok(())
}

#[test]
fn predicates_are_rejected_with_guidance() {
    let rules = r#"
(rule (m x)
  (match rust "((identifier) @x (#eq? @x \"foo\"))"))
"#;
    let result = analyze(rules, &[]);
    let Err(error) = result else {
        panic!("predicate query must be rejected");
    };
    let message = error.to_string();
    assert!(message.contains("predicates"), "got: {message}");
}

#[test]
fn unknown_relation_is_a_load_error() {
    let rules = "(rule (a x) (no-such-rel x))";
    let Err(error) = analyze(rules, &[]) else {
        panic!("unknown relation must be rejected");
    };
    assert!(error.to_string().contains("no-such-rel"));
}

#[test]
fn query_comments_are_not_predicates() -> TestResult {
    // A `;` comment may contain `#` (and `@`): tree-sitter accepts it, so the
    // predicate scan and the capture scan must skip comments rather than
    // reject the rule or invent a capture name.
    let rules = "
(rule (brk b)
  (match rust \"
    ; matches #break statements, reported as @b
    (break_expression) @b\"))
";
    let dir = tempfile::tempdir()?;
    write_sample(&dir, "sample.rs", "fn main() { loop { break; } }\n")?;
    let analysis = analyze(rules, &[dir.path().to_path_buf()])?;
    assert_eq!(analysis.database.relations["brk"].rows().len(), 1);
    Ok(())
}

#[test]
fn builtin_arity_gets_the_builtin_message() {
    let rules = "(rule (a x) (text x))";
    let Err(error) = analyze(rules, &[]) else {
        panic!("wrong-arity builtin must be rejected");
    };
    let message = error.to_string();
    assert!(
        message.contains("builtin `text` takes 2 arguments, got 1"),
        "got: {message}"
    );
}

#[test]
fn text_match_filters_by_regex() -> TestResult {
    let source = "fn main() { run(fetch_url); run(parse); run(fetch_git); }\n";
    let rules = r#"
(rule (fetchy x)
  (match rust "(call_expression arguments: (arguments (identifier) @x))")
  (text-match x "^fetch_"))
"#;
    let dir = tempfile::tempdir()?;
    write_sample(&dir, "sample.rs", source)?;
    let analysis = analyze(rules, &[dir.path().to_path_buf()])?;
    let fetchy = &analysis.database.relations["fetchy"];
    assert_eq!(fetchy.rows().len(), 2, "fetch_url and fetch_git match");
    Ok(())
}

#[test]
fn text_match_rejects_invalid_regex() -> TestResult {
    let rules = r#"
(rule (m x)
  (match rust "(identifier) @x")
  (text-match x "(unclosed"))
"#;
    let dir = tempfile::tempdir()?;
    write_sample(&dir, "sample.rs", "fn main() {}\n")?;
    let Err(error) = analyze(rules, &[dir.path().to_path_buf()]) else {
        return Err("invalid regex must be rejected".into());
    };
    let message = error.to_string();
    assert!(message.contains("invalid regex"), "got: {message}");
    assert!(message.contains("rules:4"), "got: {message}");
    Ok(())
}

#[test]
fn text_match_pattern_must_be_a_literal() {
    let rules = r#"
(rule (m x p)
  (match rust "(identifier) @x (identifier) @p")
  (text-match x p))
"#;
    let Err(error) = analyze(rules, &[]) else {
        panic!("variable text-match pattern must be rejected");
    };
    let message = error.to_string();
    assert!(message.contains("string literal"), "got: {message}");
}

#[test]
fn no_descendant_requires_kind_and_text_absence() -> TestResult {
    // Two functions: one calls danger(), one does not. `no-descendant`
    // keeps only the function whose subtree has no `danger` identifier.
    let source = "
fn clean() { safe(); }
fn dirty() { danger(); }
";
    let rules = r#"
(rule (danger-free f)
  (match rust "(function_item) @f")
  (no-descendant f "identifier" "danger"))
"#;
    let dir = tempfile::tempdir()?;
    write_sample(&dir, "sample.rs", source)?;
    let analysis = analyze(rules, &[dir.path().to_path_buf()])?;
    let rows = analysis.database.relations["danger-free"].rows();
    assert_eq!(rows.len(), 1, "only the clean function qualifies");
    let Value::Node(node) = &rows[0][0] else {
        return Err("danger-free column 0 should be a node".into());
    };
    assert!(analysis.corpus.node_text(*node).contains("clean"));
    Ok(())
}

#[test]
fn attached_sibling_scans_the_annotation_block() -> TestResult {
    // `attached-sibling` searches the annotation block directly above each def
    // (preceding siblings up to the previous def), so a `@spec` counts even with a
    // `@doc` or comment between it and the def. `(not (attached-sibling ...))`
    // then keeps only the def with no `@spec` in its block.
    let source = "
defmodule Demo do
  @spec documented() :: :ok
  def documented(), do: :ok

  @spec with_doc() :: :ok
  @doc \"a doc\"
  def with_doc(), do: :ok

  @spec with_comment() :: :ok
  # an explanatory comment
  def with_comment(), do: :ok

  def undocumented(), do: :ok
end
";
    let rules = r#"
(rule (a-def c) (match elixir "(call (identifier) @i) @c") (text i "def"))
(rule (spec-attached c) (a-def c) (attached-sibling c "identifier" "spec"))
(rule (undocumented-def c) (a-def c) (not (spec-attached c)))
"#;
    let dir = tempfile::tempdir()?;
    write_sample(&dir, "sample.ex", source)?;
    let analysis = analyze(rules, &[dir.path().to_path_buf()])?;
    let rows = analysis.database.relations["undocumented-def"].rows();
    assert_eq!(
        rows.len(),
        1,
        "only the def with no @spec in its annotation block qualifies"
    );
    let Value::Node(node) = &rows[0][0] else {
        return Err("undocumented-def column 0 should be a node".into());
    };
    assert!(analysis.corpus.node_text(*node).contains("undocumented"));
    Ok(())
}

#[test]
fn negation_filters_by_name_join() -> TestResult {
    // `keep` is a function that defines a spec for its own name; `drop` is spec'd
    // for a *different* name. A name-matched negation flags only `drop`.
    let source = "
defmodule Demo do
  @spec keep(integer()) :: integer()
  def keep(x), do: x

  @spec other() :: :ok
  def drop(y), do: y
end
";
    let rules = r#"
(rule (def-name d n)
  (match elixir "(call (identifier) @kw (arguments (call (identifier) @n))) @d")
  (text kw "def"))
(rule (spec-name n)
  (match elixir "(unary_operator (call (identifier) @kw (arguments (binary_operator (call (identifier) @sn))))) ")
  (text kw "spec")
  (text sn n))
(rule (unspecced d)
  (def-name d n)
  (text n t)
  (not (spec-name t)))
"#;
    let dir = tempfile::tempdir()?;
    write_sample(&dir, "sample.ex", source)?;
    let analysis = analyze(rules, &[dir.path().to_path_buf()])?;
    let rows = analysis.database.relations["unspecced"].rows();
    assert_eq!(rows.len(), 1, "only the def with no same-named @spec qualifies");
    let Value::Node(node) = &rows[0][0] else {
        return Err("unspecced column 0 should be a node".into());
    };
    assert!(analysis.corpus.node_text(*node).contains("drop"));
    Ok(())
}

#[test]
fn unsafe_negation_is_rejected() {
    // `y` appears only inside the negated atom, with nothing to filter against.
    let rules = r#"
(rule (thing x) (match rust "(identifier) @x"))
(rule (bad x) (match rust "(identifier) @x") (not (thing y)))
"#;
    let Err(error) = analyze(rules, &[]) else {
        panic!("negation over an unbound variable must be rejected");
    };
    let message = error.to_string();
    assert!(
        message.contains("only inside `(not ...)`"),
        "expected unsafe-negation error, got: {message}"
    );
}

#[test]
fn negation_before_its_binding_is_rejected() {
    // `t` is bound by `(text n t)` only AFTER the negation. The evaluator runs
    // left-to-right, so `t` would be unbound when `(not ...)` runs and bound
    // existentially per row instead of filtering -- safety must reject it.
    let rules = r#"
(rule (named n) (match rust "(identifier) @n"))
(rule (other t) (match rust "(identifier) @x") (text x t))
(rule (bad n) (named n) (not (other t)) (text n t))
"#;
    let Err(error) = analyze(rules, &[]) else {
        panic!("negation before its binding atom must be rejected");
    };
    assert!(
        error.to_string().contains("only inside"),
        "expected unsafe-negation error, got: {error}"
    );
}

#[test]
fn rewrite_negation_safety_is_checked() {
    // The same range-restriction applies to rewrite bodies, not just rules.
    let rules = r#"
(rule (thing x) (match rust "(identifier) @x"))
(rewrite r (match rust "(identifier) @x") (not (thing y)) (replace x "z"))
"#;
    let Err(error) = analyze(rules, &[]) else {
        panic!("unbound negation in a rewrite body must be rejected");
    };
    assert!(
        error.to_string().contains("only inside"),
        "expected unsafe-negation error, got: {error}"
    );
}

#[test]
fn negation_through_recursion_is_rejected() {
    let rules = r#"
(rule (p x) (match rust "(identifier) @x"))
(rule (p x) (match rust "(identifier) @x") (not (p x)))
"#;
    let Err(error) = analyze(rules, &[]) else {
        panic!("negation through recursion must be rejected");
    };
    let message = error.to_string();
    assert!(
        message.contains("stratif") || message.contains("recursion"),
        "expected unstratifiable error, got: {message}"
    );
}

/// One rule flagging every `.unwrap()` receiver, lint-declared as an error
/// with a `{e}` splice.
const LINT_RULES: &str = r#"
(rule (unwrap-call call e)
  (match rust "
    (call_expression
      function: (field_expression value: (_) @e field: (field_identifier) @m)
      arguments: (arguments)) @call")
  (text m "unwrap"))

(lint unwrap-call error "do not unwrap `{e}`")
"#;

#[test]
fn lint_unknown_relation_is_a_load_error() {
    let rules = r#"(lint no-such-rel error "boom")"#;
    let Err(error) = analyze(rules, &[]) else {
        panic!("lint on an unknown relation must be rejected");
    };
    let message = error.to_string();
    assert!(message.contains("no-such-rel"), "got: {message}");
    assert!(message.contains("rules:1"), "got: {message}");
}

#[test]
fn lint_bad_severity_is_a_typed_error() {
    let rules = r#"
(rule (brk b) (match rust "(break_expression) @b"))
(lint brk fatal "no breaks")
"#;
    let Err(error) = analyze(rules, &[]) else {
        panic!("bad lint severity must be rejected");
    };
    let crate::Error::LintSeverity { got, line } = error else {
        panic!("expected LintSeverity, got: {error}");
    };
    assert_eq!(got, "fatal");
    assert_eq!(line, 3);
}

#[test]
fn lint_template_var_must_be_a_head_variable() {
    let rules = r#"
(rule (brk b) (match rust "(break_expression) @b"))
(lint brk error "break at {other}")
"#;
    let Err(error) = analyze(rules, &[]) else {
        panic!("lint message variable outside the head must be rejected");
    };
    let crate::Error::LintVar { relation, var, .. } = error else {
        panic!("expected LintVar, got: {error}");
    };
    assert_eq!(relation, "brk");
    assert_eq!(var, "other");
}

#[test]
fn scan_emits_located_findings_with_spliced_message() -> TestResult {
    let dir = tempfile::tempdir()?;
    write_sample(&dir, "sample.rs", RUST_SAMPLE)?;
    let analysis = analyze(LINT_RULES, &[dir.path().to_path_buf()])?;
    let findings = analysis.findings()?;
    assert_eq!(findings.len(), 2, "both unwrap sites become findings");
    let first = &findings[0];
    assert_eq!(first.rule, "unwrap-call");
    assert_eq!(first.severity, Severity::Error);
    assert_eq!(first.message, "do not unwrap `compute()`");
    assert_eq!(first.text, "compute().unwrap()");
    assert_eq!((first.line, first.column), (3, 13));
    assert!(first.end_line == 3 && first.end_column > first.column);
    assert!(
        findings.windows(2).all(|pair| {
            (&pair[0].file, pair[0].line) <= (&pair[1].file, pair[1].line)
        }),
        "findings are sorted by position"
    );
    Ok(())
}

#[test]
fn scan_requires_a_node_column() -> TestResult {
    // The lint relation's only column is derived text, so a finding has no
    // location: a typed error, not a panic.
    let rules = r#"
(rule (kinds k)
  (match rust "(break_expression) @b")
  (kind b k))
(lint kinds error "saw kind {k}")
"#;
    let dir = tempfile::tempdir()?;
    write_sample(&dir, "sample.rs", "fn main() { loop { break; } }\n")?;
    let analysis = analyze(rules, &[dir.path().to_path_buf()])?;
    let Err(error) = analysis.findings() else {
        return Err("a lint row without a node column must fail scan".into());
    };
    let crate::Error::LintNoNode { rule, .. } = error else {
        return Err(format!("expected LintNoNode, got: {error}").into());
    };
    assert_eq!(rule, "kinds");
    Ok(())
}

#[test]
fn suppression_filters_same_line_and_line_below() -> TestResult {
    let source = "
fn f() -> u32 {
    a().unwrap(); // astlog-ignore
    // astlog-ignore
    b().unwrap();
    c().unwrap();
    0
}
";
    let dir = tempfile::tempdir()?;
    write_sample(&dir, "sample.rs", source)?;
    let analysis = analyze(LINT_RULES, &[dir.path().to_path_buf()])?;
    let findings = analysis.findings()?;
    assert_eq!(findings.len(), 1, "only the uncommented site survives");
    assert_eq!(findings[0].text, "c().unwrap()");
    // The rows themselves are untouched: suppression filters emission only.
    assert_eq!(analysis.database.relations["unwrap-call"].rows().len(), 3);
    Ok(())
}

#[test]
fn suppressed_reports_the_originating_comment() -> TestResult {
    // The trailing comment suppresses the unwrap on its own line; `findings`
    // omits that row and `suppressed` reports it with the comment that hid it.
    // A blank line below the suppressed site keeps the trailing comment's
    // line-below coverage off the second unwrap.
    let source = "
fn f() -> u32 {
    a().unwrap(); // astlog-ignore: unwrap-call (legacy, drop me)

    b().unwrap();
    0
}
";
    let dir = tempfile::tempdir()?;
    write_sample(&dir, "sample.rs", source)?;
    let analysis = analyze(LINT_RULES, &[dir.path().to_path_buf()])?;

    let findings = analysis.findings()?;
    assert_eq!(findings.len(), 1, "only the uncommented site is reported");
    assert_eq!(findings[0].text, "b().unwrap()");

    let suppressed = analysis.suppressed()?;
    assert_eq!(suppressed.len(), 1, "exactly the hidden site is listed");
    let only = &suppressed[0];
    assert_eq!(only.finding.text, "a().unwrap()");
    assert_eq!(only.finding.line, 3);
    assert_eq!(only.comment_line, 3, "trailing comment shares the row's line");
    assert_eq!(
        only.comment_text,
        "// astlog-ignore: unwrap-call (legacy, drop me)"
    );
    Ok(())
}

#[test]
fn named_suppression_matches_only_its_rules() -> TestResult {
    // Blank lines isolate each site: a trailing suppression also covers the
    // line below it, and these cases must not bleed into each other.
    let source = "
fn f() -> u32 {
    a().unwrap(); // astlog-ignore: unwrap-call

    b().unwrap(); // astlog-ignore: some-other-rule

    c().unwrap(); // astlog-ignore: some-other-rule, unwrap-call

    0
}
";
    let dir = tempfile::tempdir()?;
    write_sample(&dir, "sample.rs", source)?;
    let analysis = analyze(LINT_RULES, &[dir.path().to_path_buf()])?;
    let findings = analysis.findings()?;
    assert_eq!(
        findings.len(),
        1,
        "the wrong-name suppression must not suppress"
    );
    assert_eq!(findings[0].text, "b().unwrap()");
    Ok(())
}

#[test]
fn bare_suppression_reports_comment_via_all_directive() -> TestResult {
    // A bare `astlog-ignore` (no rule list) takes the `Directive::All` branch,
    // which records the originating comment separately from the named path.
    // `suppressed` must still report that comment so an audit shows what an
    // unscoped ignore is hiding.
    let source = "
fn f() -> u32 {
    a().unwrap(); // astlog-ignore

    b().unwrap();
    0
}
";
    let dir = tempfile::tempdir()?;
    write_sample(&dir, "sample.rs", source)?;
    let analysis = analyze(LINT_RULES, &[dir.path().to_path_buf()])?;

    let suppressed = analysis.suppressed()?;
    assert_eq!(suppressed.len(), 1, "exactly the hidden site is listed");
    let only = &suppressed[0];
    assert_eq!(only.finding.text, "a().unwrap()");
    assert_eq!(only.comment_line, 3);
    assert_eq!(only.comment_text, "// astlog-ignore");
    Ok(())
}

#[test]
fn warning_severity_is_carried_through() -> TestResult {
    let rules = r#"
(rule (brk b) (match rust "(break_expression) @b"))
(lint brk warning "loop break")
"#;
    let dir = tempfile::tempdir()?;
    write_sample(&dir, "sample.rs", "fn main() { loop { break; } }\n")?;
    let analysis = analyze(rules, &[dir.path().to_path_buf()])?;
    let findings = analysis.findings()?;
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].severity, Severity::Warning);
    assert_eq!(findings[0].message, "loop break");
    Ok(())
}

#[test]
fn no_descendant_is_strict() -> TestResult {
    // The root node itself never counts as its own descendant, even when
    // kind and text both coincide.
    let source = "fn main() { x; }\n";
    let rules = r#"
(rule (ident-without-self x)
  (match rust "(identifier) @x")
  (text x "x")
  (no-descendant x "identifier" "x"))
"#;
    let dir = tempfile::tempdir()?;
    write_sample(&dir, "sample.rs", source)?;
    let analysis = analyze(rules, &[dir.path().to_path_buf()])?;
    let rows = analysis.database.relations["ident-without-self"].rows();
    assert_eq!(rows.len(), 1, "the identifier has no matching descendant");
    Ok(())
}
