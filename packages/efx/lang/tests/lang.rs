use efx_ir::{Literal, Value};
use efx_lang::compile;

const HAPPY: &str = r#"
# A plan with a binding, interpolation, a ref, and metadata.
let title = "efx demo"
let repeat = 3

effect greeting "cmd.run" {
  @idempotent = false
  command = "echo hello"
}

effect page "html.render" {
  template = "<h1>{title}</h1> x{repeat} {{literal}}"
  body = ref("greeting").stdout
}
"#;

#[test]
fn happy_path_compiles_to_a_plan() {
    let plan = compile(HAPPY).expect("compiles");
    assert_eq!(plan.effects().len(), 2);

    let greeting = plan.get("greeting").unwrap();
    assert_eq!(greeting.kind, "cmd.run");
    assert_eq!(greeting.executor, "cmd.run");
    assert!(!greeting.meta.idempotent);

    let page = plan.get("page").unwrap();
    assert_eq!(
        page.inputs["template"],
        Value::Literal(Literal::Str("<h1>efx demo</h1> x3 {literal}".into()))
    );
    let Value::Ref(body) = &page.inputs["body"] else {
        panic!("body wires an upstream output");
    };
    assert_eq!(body.effect, "greeting");
    assert_eq!(body.field, "stdout");

    let edges = plan.edges();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].from, "greeting");
    assert_eq!(edges[0].to, "page");
}

#[test]
fn same_source_compiles_to_same_ids() {
    let first = compile(HAPPY).unwrap().effect_ids().unwrap();
    let second = compile(HAPPY).unwrap().effect_ids().unwrap();
    assert_eq!(first, second);
}

fn error_of(source: &str) -> efx_lang::CompileError {
    compile(source).expect_err("must not compile")
}

#[test]
fn missing_equals_is_located() {
    let err = error_of("effect a \"cmd.run\" {\n  command \"echo\"\n}\n");
    assert_eq!((err.line, err.col), (2, 11));
    assert!(
        err.message.contains("expected `=` after the input name"),
        "{err}"
    );
}

#[test]
fn unknown_binding_is_located() {
    let err = error_of("effect a \"cmd.run\" {\n  command = nope\n}\n");
    assert_eq!((err.line, err.col), (2, 13));
    assert!(
        err.message
            .contains("`nope` is not defined by an earlier `let`"),
        "{err}"
    );
}

#[test]
fn unknown_interpolation_names_the_binding() {
    let err = error_of("effect a \"cmd.run\" {\n  command = \"echo {ghost}\"\n}\n");
    assert!(err.message.contains("{ghost}"), "{err}");
    assert!(err.message.contains("no `let ghost`"), "{err}");
}

#[test]
fn unterminated_string_is_reported() {
    let err = error_of("let a = \"oops\n");
    assert!(err.message.contains("unterminated string"), "{err}");
}

#[test]
fn ref_to_undeclared_effect_is_reported() {
    let err = error_of("effect a \"cmd.run\" {\n  command = ref(\"ghost\").stdout\n}\n");
    assert!(err.message.contains("`ref(\"ghost\")`"), "{err}");
}

#[test]
fn lets_cannot_see_later_lets() {
    // Bindings are strictly ordered — this is what keeps the language total.
    let err = error_of("let a = \"{b}\"\nlet b = \"x\"\n");
    assert!(err.message.contains("no `let b`"), "{err}");
}

#[test]
fn render_points_at_the_offending_line() {
    let source = "effect a \"cmd.run\" {\n  command \"echo\"\n}\n";
    let rendered = error_of(source).render(source);
    assert!(rendered.contains("  command \"echo\""), "{rendered}");
    assert!(rendered.lines().last().unwrap_or_default().ends_with('^'));
}
