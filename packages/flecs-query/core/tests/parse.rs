//! Conformance tests, derived from `docs/FlecsQueryLanguage.md` and the
//! shapes exercised by flecs's `test/query/src/Parser.c`.

use flecs_query_core::{
    Access, EqOp, EqOperand, ExtraOper, IdFlag, Oper, Query, Ref, RefExpr, Src, Term, TermBody,
    Traversal, parse,
};

/// Parse, assert the term count, and assert the canonical form round-trips
/// to the same AST.
fn roundtrip(expr: &str, terms: usize) -> Query {
    let query = parse(expr).unwrap_or_else(|error| {
        panic!("{}", error.render(expr));
    });
    assert_eq!(query.terms.len(), terms, "term count of {expr:?}");
    let canonical = query.to_string();
    let reparsed = parse(&canonical).unwrap_or_else(|error| {
        panic!(
            "canonical form of {expr:?} does not reparse: {canonical:?}\n{rendered}",
            rendered = error.render(&canonical)
        );
    });
    assert_eq!(reparsed, query, "round-trip of {expr:?} via {canonical:?}");
    query
}

/// Parse, expecting a syntax error.
fn reject(expr: &str) {
    assert!(
        parse(expr).is_err(),
        "expected {expr:?} to be rejected, parsed as {:?}",
        parse(expr)
    );
}

fn id(term: &Term) -> &flecs_query_core::IdTerm {
    match &term.body {
        TermBody::Id(id) => id,
        other => panic!("expected an id term, got {other:?}"),
    }
}

fn name(text: &str) -> RefExpr {
    RefExpr::Name(text.to_owned())
}

#[test]
fn empty_query_forms() {
    assert!(parse("0").unwrap().terms.is_empty());
    assert!(parse("").unwrap().terms.is_empty());
    assert!(parse("\n").unwrap().terms.is_empty());
    assert!(parse(" // just a comment").unwrap().terms.is_empty());
    assert!(parse("/* nothing */").unwrap().terms.is_empty());
    assert_eq!(parse("0").unwrap().to_string(), "0");
}

#[test]
fn components() {
    let query = roundtrip("Position, Velocity", 2);
    assert_eq!(id(&query.terms[0]).first.expr, name("Position"));
    assert_eq!(id(&query.terms[0]).src, Src::Implicit);
    assert_eq!(query.to_string(), "Position, Velocity");
}

#[test]
fn trailing_separators_and_whitespace() {
    roundtrip("Position,", 1);
    roundtrip("  Position  ", 1);
    roundtrip("Position\n", 1);
    roundtrip("Position, // comment\nVelocity", 2);
    roundtrip("Position /* inline */, Velocity", 2);
}

#[test]
fn newline_is_whitespace_not_a_separator() {
    // Upstream query parsing treats newlines as insignificant.
    roundtrip("Position,\nVelocity", 2);
    roundtrip("Position(\n  $this\n)", 1);
    reject("Position\nVelocity");
}

#[test]
fn pairs() {
    let query = roundtrip("(Likes, Bob), (Eats, Apples)", 2);
    let likes = id(&query.terms[0]);
    assert_eq!(likes.first.expr, name("Likes"));
    assert_eq!(likes.src, Src::Implicit);
    assert_eq!(likes.second.as_ref().unwrap().expr, name("Bob"));
    assert_eq!(query.to_string(), "(Likes, Bob), (Eats, Apples)");
}

#[test]
fn explicit_sources() {
    let query = roundtrip("TimeOfDay(Game), Position($this), Likes($this, Dogs)", 3);
    assert_eq!(id(&query.terms[0]).src, Src::Explicit(Ref::plain(name("Game"))));
    assert_eq!(id(&query.terms[1]).src, Src::Explicit(Ref::plain(RefExpr::This)));
    let likes = id(&query.terms[2]);
    assert_eq!(likes.second.as_ref().unwrap().expr, name("Dogs"));
}

#[test]
fn empty_and_zero_sources() {
    let query = roundtrip("Position(), Position(#0)", 2);
    assert_eq!(id(&query.terms[0]).src, Src::Empty);
    assert_eq!(
        id(&query.terms[1]).src,
        Src::Explicit(Ref::plain(RefExpr::Entity(0)))
    );
}

#[test]
fn operators() {
    let query = roundtrip("Position, !Velocity, ?Mass", 3);
    assert_eq!(query.terms[0].oper, Oper::And);
    assert_eq!(query.terms[1].oper, Oper::Not);
    assert_eq!(query.terms[2].oper, Oper::Optional);
}

#[test]
fn or_chains() {
    let query = roundtrip("Position, Velocity || Mass, Rotation", 4);
    assert_eq!(query.terms[1].oper, Oper::Or);
    assert_eq!(query.terms[2].oper, Oper::And);
    assert_eq!(query.to_string(), "Position, Velocity || Mass, Rotation");

    roundtrip("(Likes, Cats) || (Likes, Dogs)", 2);
    roundtrip("Position || Velocity || Mass", 3);
    // The right-hand side of `||` may carry its own operator...
    roundtrip("Position || !Velocity", 2);
    roundtrip("Position || ?Velocity", 2);
    // ...but the left-hand side may not.
    reject("!Position || Velocity");
    reject("?Position || Velocity");
}

#[test]
fn from_operators() {
    let query = roundtrip("Position, and|MyType, or|MyType, not|MyType", 4);
    assert_eq!(query.terms[1].oper, Oper::AndFrom);
    assert_eq!(query.terms[2].oper, Oper::OrFrom);
    assert_eq!(query.terms[3].oper, Oper::NotFrom);
    // The pipe after the keyword is mandatory: these words are reserved at
    // the start of a term.
    reject("and");
    reject("not, Position");
}

#[test]
fn id_flags() {
    let query = roundtrip("auto_override|Position, toggle|Velocity", 2);
    assert_eq!(id(&query.terms[0]).flag, Some(IdFlag::AutoOverride));
    assert_eq!(id(&query.terms[1]).flag, Some(IdFlag::Toggle));
    roundtrip("auto_override|(Rel, Tgt)", 1);
}

#[test]
fn access_modifiers() {
    let query = roundtrip(
        "[inout] Position, [in] Velocity, [out] Mass, [none] Rotation, [filter] Scale, [default] X",
        6,
    );
    let access: Vec<_> = query.terms.iter().map(|term| term.access).collect();
    assert_eq!(
        access,
        [
            Some(Access::InOut),
            Some(Access::In),
            Some(Access::Out),
            Some(Access::None),
            Some(Access::Filter),
            Some(Access::Default),
        ]
    );
    roundtrip("[in] !Position", 1);
    roundtrip("[in] ?Position", 1);
    roundtrip("[in] (Likes, Dogs)", 1);
    // Deliberately stricter than upstream, which ignores unknown modifiers.
    reject("[frobnicate] Position");
}

#[test]
fn wildcards() {
    let query = roundtrip("Position, (Likes, *), (*, Dogs), (_, _), *", 5);
    assert_eq!(
        id(&query.terms[1]).second.as_ref().unwrap().expr,
        RefExpr::Wildcard
    );
    assert_eq!(id(&query.terms[3]).first.expr, RefExpr::Any);
    assert_eq!(id(&query.terms[4]).first.expr, RefExpr::Wildcard);
}

#[test]
fn variables() {
    let query = roundtrip("(Likes, $food), (Eats, $food), Serializable($component)", 3);
    assert_eq!(
        id(&query.terms[0]).second.as_ref().unwrap().expr,
        RefExpr::Var("food".to_owned())
    );
    assert_eq!(
        id(&query.terms[2]).src,
        Src::Explicit(Ref::plain(RefExpr::Var("component".to_owned())))
    );

    // `$this` is the builtin variable, `this` is a plain name, `$` is the
    // singleton source.
    let query = roundtrip("Position($this), Position(this), Position($)", 3);
    assert_eq!(id(&query.terms[0]).src, Src::Explicit(Ref::plain(RefExpr::This)));
    assert_eq!(id(&query.terms[1]).src, Src::Explicit(Ref::plain(name("this"))));
    assert_eq!(
        id(&query.terms[2]).src,
        Src::Explicit(Ref::plain(RefExpr::Var(String::new())))
    );

    // Variables can appear in any position, including as the component.
    roundtrip("$component($this)", 1);
    roundtrip("SpaceShip($this), DockedTo($this, $planet), Planet($planet)", 3);
}

#[test]
fn lookup_variables() {
    // `$this.cockpit` is one identifier: a lookup relative to the variable.
    let query = roundtrip("SpaceShip($this), !Powered($this.cockpit)", 2);
    assert_eq!(
        id(&query.terms[1]).src,
        Src::Explicit(Ref::plain(RefExpr::Var("this.cockpit".to_owned())))
    );
}

#[test]
fn entity_ids() {
    let query = roundtrip("#511, (#510, #511), Position(#512)", 3);
    assert_eq!(id(&query.terms[0]).first.expr, RefExpr::Entity(511));
    // A bare number is also an entity id; it canonicalizes to `#id`.
    let query = roundtrip("524288", 1);
    assert_eq!(id(&query.terms[0]).first.expr, RefExpr::Entity(524_288));
    assert_eq!(query.to_string(), "#524288");
    reject("#notanumber");
}

#[test]
fn lookup_paths_and_members() {
    roundtrip("flecs.meta.Member", 1);
    let query = roundtrip("(Movement.direction, Left), Movement.direction($this, Left)", 2);
    assert_eq!(id(&query.terms[0]).first.expr, name("Movement.direction"));
    // `\.` keeps a literal dot inside one name.
    let query = roundtrip("foo\\.bar", 1);
    assert_eq!(id(&query.terms[0]).first.expr, name("foo\\.bar"));
    assert_eq!(query.to_string(), "foo\\.bar");
    // `.*` wildcard lookup suffix stays part of the identifier.
    roundtrip("(ns.*, Tgt)", 1);
}

#[test]
fn escaped_names() {
    let query = roundtrip("Tag\\ Name", 1);
    assert_eq!(id(&query.terms[0]).first.expr, name("Tag Name"));
    assert_eq!(query.to_string(), "Tag\\ Name");
}

#[test]
fn template_types() {
    let query = roundtrip("Position<int>, Map<string, vector<int>>", 2);
    assert_eq!(id(&query.terms[0]).first.expr, name("Position<int>"));
    assert_eq!(
        id(&query.terms[1]).first.expr,
        name("Map<string, vector<int>>")
    );
    reject("Position<int");
    reject("Position>int");
}

#[test]
fn traversal() {
    let query = roundtrip(
        "Transform, Transform(up ChildOf), Transform(up), Transform(cascade), Transform(cascade|desc), Style(self|up)",
        6,
    );
    let up_childof = id(&query.terms[1]);
    let Src::Explicit(src) = &up_childof.src else {
        panic!("expected explicit source");
    };
    assert_eq!(src.expr, RefExpr::Implied);
    assert_eq!(
        src.traversal,
        Some(Traversal {
            up: true,
            rel: Some("ChildOf".to_owned()),
            ..Traversal::default()
        })
    );

    // Flags on an explicit source entity, on the component, and on a pair
    // target.
    roundtrip("Position($this|up ChildOf)", 1);
    roundtrip("Unit|self", 1);
    roundtrip("Unit|self($this)", 1);
    roundtrip("(Position|self, Tgt)", 1);
    roundtrip("(Rel, Tgt|self)", 1);
    roundtrip("LocatedIn($this, SanFrancisco|self)", 1);
    roundtrip("Position(self|up IsA)", 1);

    reject("Position|frobnicate");
    reject("Position(src|frobnicate)");
}

#[test]
fn equality_predicates() {
    let query = roundtrip(
        "SpaceShip($this), $this == UssEnterprise || $this == Voyager",
        3,
    );
    let TermBody::Eq(eq) = &query.terms[1].body else {
        panic!("expected an equality term");
    };
    assert_eq!(eq.left, RefExpr::This);
    assert_eq!(eq.op, EqOp::Eq);
    assert_eq!(eq.right, EqOperand::Ref(name("UssEnterprise")));
    assert_eq!(query.terms[1].oper, Oper::Or);

    let query = roundtrip("$this != UssEnterprise", 1);
    assert_eq!(query.terms[0].oper, Oper::Not);
    assert_eq!(query.to_string(), "$this != UssEnterprise");

    roundtrip("PoweredBy($this, $source), $this != $source", 2);
    roundtrip("$this == \"UssEnterprise\"", 1);
    roundtrip("$this == *", 1);
    roundtrip("$this == _", 1);
    roundtrip("$this == #0", 1);

    let query = roundtrip("$this ~= \"Uss\"", 1);
    assert_eq!(query.terms[0].oper, Oper::And);

    // `~= "!..."` embeds the negation in the string.
    let query = roundtrip("$this ~= \"!Uss\"", 1);
    assert_eq!(query.terms[0].oper, Oper::Not);
    let TermBody::Eq(eq) = &query.terms[0].body else {
        panic!("expected an equality term");
    };
    assert_eq!(eq.right, EqOperand::Name("Uss".to_owned()));
    assert_eq!(query.to_string(), "$this ~= \"!Uss\"");

    // Equality cannot combine with a `!`/`?` prefix or a keyword operator.
    reject("!$this == Foo");
    reject("?$this == Foo");
    reject("and|$this == Foo");
}

#[test]
fn query_scopes() {
    let query = roundtrip("SpaceShip, !{ (Engine, $engine), Healthy($engine) }", 2);
    assert_eq!(query.terms[1].oper, Oper::Not);
    let TermBody::Scope(inner) = &query.terms[1].body else {
        panic!("expected a scope");
    };
    assert_eq!(inner.len(), 2);
    assert_eq!(
        query.to_string(),
        "SpaceShip, !{(Engine, $engine), Healthy($engine)}"
    );

    roundtrip("TagA, {TagB}", 2);
    roundtrip("TagA, !{TagB, !{TagC}}", 2);
    roundtrip("TagA, {\nTagB\n}", 2);
    roundtrip("TagA, {TagB}, TagC", 3);
    reject("TagA, !{TagB");
    reject("TagA, TagB}");
    reject("[in] {TagB}");
}

#[test]
fn multi_target_pairs() {
    let query = roundtrip("Rel($this, A, B)", 1);
    let rel = id(&query.terms[0]);
    assert_eq!(rel.second.as_ref().unwrap().expr, name("A"));
    assert_eq!(rel.extra, [Ref::plain(name("B"))]);
    assert_eq!(rel.extra_oper, ExtraOper::And);

    let query = roundtrip("(Rel, A || B)", 1);
    assert_eq!(id(&query.terms[0]).extra_oper, ExtraOper::Or);

    roundtrip("Rel($this, A, B, C)", 1);
    roundtrip("(Rel, A, B)", 1);
    roundtrip("Rel($this, A || B || C)", 1);
    roundtrip("Rel($this, $a, $b)", 1);

    // `,` and `||` cannot mix between extra targets.
    reject("Rel($this, A || B, C)");
    reject("Rel($this, A, B || C)");
}

#[test]
fn value_operands() {
    let query = roundtrip("(Color, @Red), (Color, @*), (Color, @7)", 3);
    assert_eq!(
        id(&query.terms[0]).second.as_ref().unwrap().expr,
        RefExpr::Value(Box::new(name("Red")))
    );
}

#[test]
fn singleton_trait() {
    roundtrip("Position($), Velocity", 2);
    assert_eq!(roundtrip("Position($)", 1).to_string(), "Position($)");
}

#[test]
fn anonymous_variables() {
    let query = roundtrip("(Likes, $_food)", 1);
    assert_eq!(
        id(&query.terms[0]).second.as_ref().unwrap().expr,
        RefExpr::Var("_food".to_owned())
    );
}

#[test]
fn malformed_expressions() {
    reject("Position, Velocity)");
    reject("Position(");
    reject("Position($this");
    reject("Position($this,");
    reject("(Position");
    reject("(Position)");
    reject("Position,, Velocity");
    reject(", Position");
    reject("Position,\n, Velocity");
    reject("Position ||");
    reject("[in Position");
    reject("[in] |Position");
    reject("Position | Velocity");
    reject("$this ==");
    reject("\"unterminated");
    reject("/* unterminated");
    reject("Position & Velocity");
}

#[test]
fn canonical_display_is_stable() {
    // Display output parses back to itself byte-for-byte (a fixpoint).
    for expr in [
        "Position, [in] Velocity, !Dead, (ChildOf, $parent), Planet($parent)",
        "Transform, Transform(cascade|desc IsA)",
        "SpaceShip, !{(Engine, $e), Healthy($e)}",
        "and|MyType, auto_override|Tag, $this ~= \"!Uss\"",
        "Rel($this, A || B)",
    ] {
        let query = parse(expr).unwrap();
        let canonical = query.to_string();
        assert_eq!(parse(&canonical).unwrap().to_string(), canonical);
    }
}

#[cfg(feature = "serde")]
#[test]
fn ast_serializes() {
    let query = parse("Position, (ChildOf, $parent)").unwrap();
    let json = serde_json::to_string(&query).unwrap();
    let back: Query = serde_json::from_str(&json).unwrap();
    assert_eq!(back, query);
}

#[test]
fn multibyte_escapes_stay_intact() {
    // `\é` in an identifier keeps the full char and byte positions in sync
    // (a fixed 2-byte advance used to split the codepoint and corrupt both).
    let query = roundtrip("Fo\\éo", 1);
    assert_eq!(id(&query.terms[0]).first.expr, name("Foéo"));

    // Same in string operands.
    let query = roundtrip("$this == \"\\é\"", 1);
    let TermBody::Eq(eq) = &query.terms[0].body else {
        panic!("expected an equality term");
    };
    assert_eq!(eq.right, EqOperand::Name("é".to_owned()));

    // A trailing backslash ends the identifier instead of reading past it.
    roundtrip("Foo\\", 1);
}

#[test]
fn errors_on_multibyte_input_never_panic() {
    // Unknown multi-byte tokens surface whole, and rendering the error must
    // not slice mid-codepoint.
    for expr in ["é", "A\\é", "Position, ✨", "$this == \"é"] {
        if let Err(error) = parse(expr) {
            let rendered = error.render(expr);
            assert!(rendered.contains('^'), "no caret for {expr:?}: {rendered}");
        }
    }
    let error = parse("é").unwrap_err();
    assert!(error.message.contains('é'), "got: {}", error.message);
}

#[test]
fn render_snaps_arbitrary_spans_to_char_boundaries() {
    // `render` accepts any (span, src) pair; a mid-codepoint span must not
    // panic the caret slicing.
    let error = flecs_query_core::ParseError {
        message: "boom".to_owned(),
        span: flecs_query_core::Span { start: 3, end: 3 },
    };
    assert!(error.render("ab\u{e9}cd").contains('^'));
}

#[test]
fn deep_scope_nesting_is_an_error_not_a_stack_overflow() {
    // 100k nested scopes used to abort the process via unbounded recursion.
    let bomb = "{".repeat(100_000);
    let error = parse(&bomb).unwrap_err();
    assert!(error.message.contains("nested"), "got: {}", error.message);

    // Balanced nesting under the limit still works.
    let depth = 32;
    let nested = format!("{}TagA{}", "!{".repeat(depth), "}".repeat(depth));
    roundtrip(&nested, 1);
}

#[test]
fn template_arguments_keep_multibyte_chars() {
    // The <...> branch used to copy byte-wise, turning é into mojibake.
    let query = roundtrip("Rel<Posé>", 1);
    assert_eq!(id(&query.terms[0]).first.expr, name("Rel<Posé>"));
    roundtrip("Map<string, vé<int>>", 1);
}
