//! Source positions survive from the CST to the two places they are observed:
//! `builtins.unsafeGetAttrPos`, and the position on a failing evaluation.
//!
//! ENG-12137. Every expectation here was read off cppnix
//! (`nix-instantiate --eval --strict`, system nix 2.32) on a file under
//! `/private/tmp` -- not `/tmp`, which on macOS is a symlink cppnix does not
//! resolve, so it silently loses the source and reports column `offset + 1`
//! on line 1 for everything. See `maintainers/ix/positions.md`.

use nix_eval_rs::compile::Origin;
use nix_eval_rs::eval::{eval_str, eval_str_at};

/// Evaluate `src` as if it were the file at `path`.
fn at_file(src: &str, path: &str) -> String {
    match eval_str_at(src, "/private/tmp", Origin::File(path)) {
        Ok(text) => text,
        Err(error) => format!("{error:?}"),
    }
}

const F: &str = "/private/tmp/pos/f.nix";

/// `builtins.unsafeGetAttrPos "<name>" (<set>)` on one line of a file. The
/// prefix is 31 bytes for a one-character name, so a column here is countable
/// by hand off the fixture and is compared against the oracle rows in
/// `maintainers/ix/positions.md`.
fn pos_of(name: &str, set: &str) -> String {
    at_file(&format!("builtins.unsafeGetAttrPos \"{name}\" ({set})"), F)
}

/// `{ column = C; file = F; line = 1; }`, the answer's printed form.
fn col(c: u32) -> String {
    format!(r#"{{ column = {c}; file = "{F}"; line = 1; }}"#)
}

// -- the base case ----------------------------------------------------------

/// `builtins.unsafeGetAttrPos "b" ({ a = 1; b = 2; })` in a one-line file:
/// `b`'s name token is at byte offset 42, so column 43.
#[test]
fn a_literal_attribute_answers_where_its_name_was_written() {
    let src = "builtins.unsafeGetAttrPos \"b\" ({ a = 1; b = 2; })";
    assert_eq!(src.find("b = 2"), Some(40), "the fixture moved");
    assert_eq!(
        at_file(src, F),
        format!(r#"{{ column = 41; file = "{F}"; line = 1; }}"#)
    );
}

/// The position is the attribute's own, not the set's: two attributes of one
/// set answer two different columns.
#[test]
fn two_attributes_of_one_set_answer_different_columns() {
    assert_eq!(pos_of("a", "{ a = 1; b = 2; }"), col(34));
    assert_eq!(pos_of("b", "{ a = 1; b = 2; }"), col(41));
}

/// A line and a column, on a file with more than one line, so a `line_starts`
/// off-by-one cannot hide behind everything being line 1.
#[test]
fn a_multi_line_file_answers_the_right_line() {
    assert_eq!(
        at_file(
            "builtins.unsafeGetAttrPos \"b\" {\n  a = 1;\n  b = 2;\n}",
            F
        ),
        format!(r#"{{ column = 3; file = "{F}"; line = 3; }}"#)
    );
}

/// A `\r\n` file. cppnix's `Pos::LinesIterator` ends a line at `\n`, `\r\n`
/// or a bare `\r`; counting only `\n` would report line 3 column 4 here.
#[test]
fn carriage_returns_end_a_line() {
    assert_eq!(
        at_file(
            "builtins.unsafeGetAttrPos \"b\" {\r\n  a = 1;\r\n  b = 2;\r\n}",
            F
        ),
        format!(r#"{{ column = 3; file = "{F}"; line = 3; }}"#)
    );
}

/// Columns are byte offsets in cppnix, not character offsets: the `é` before
/// `b` is two bytes and moves the column by two.
#[test]
fn columns_count_bytes() {
    assert_eq!(pos_of("b", "{ b = 1; }"), col(34));
    // `é` is two bytes and one character, so `b` moves ten characters and
    // eleven bytes. A char-counting column would say 43; cppnix says 44.
    assert_eq!(
        at_file("builtins.unsafeGetAttrPos \"b\" ({ \"é\" = 0; b = 1; })", F),
        col(44)
    );
}

// -- the cases that must answer null ----------------------------------------

/// cppnix builds the record only for a `SourcePath` origin (`eval.cc`'s
/// `mkPos`), so text with no file behind it answers `null`. Confirmed:
/// `nix-instantiate --eval -E 'builtins.unsafeGetAttrPos "a" { a = 1; }'`
/// prints `null`.
#[test]
fn a_string_origin_answers_null() {
    assert_eq!(
        eval_str(r#"builtins.unsafeGetAttrPos "a" { a = 1; }"#).unwrap_or_default(),
        "null"
    );
}

#[test]
fn an_absent_attribute_answers_null() {
    assert_eq!(pos_of("zz", "{ a = 1; }"), "null");
}

/// A dynamic name is not known until the op runs, so no source token can be
/// attributed to it.
/// A dynamic name is not in the source as text, but the `${` token that
/// produces it is, and that is what cppnix records (`ExprAttrs::eval` inserts
/// each `dynamicAttrs` entry with its own `i.pos`).
#[test]
fn a_dynamic_attribute_answers_its_interpolation() {
    assert_eq!(pos_of("a", r#"{ ${"a"} = 1; }"#), col(34));
    assert_eq!(pos_of("a", r#"{ ${"z"} = 0; ${"a"} = 1; }"#), col(46));
}

/// A KNOWN DIVERGENCE, pinned so it cannot change silently. cppnix answers
/// column 69 here -- `prim_listToAttrs` copies each element's `value`
/// attribute along with that attribute's own position, and a set assembled
/// out of N elements therefore carries N unrelated positions. This crate
/// keeps one origin per set (see `maintainers/ix/positions.md`), so it has
/// nowhere to put them and answers `null`, which is a missing answer rather
/// than a wrong one.
#[test]
fn a_set_a_builtin_assembled_answers_null() {
    assert_eq!(
        pos_of(
            "a",
            r#"builtins.listToAttrs [ { name = "a"; value = 1; } ]"#
        ),
        "null"
    );
}

// -- derived sets: the origin follows the values -----------------------------

/// `//` takes the right operand's values where they collide, so it takes the
/// right operand's origin. Answering with the left's would report a position
/// for an attribute whose value came from somewhere else.
#[test]
fn update_takes_the_right_operands_origin() {
    // Column 48 is the right `a`; the left one is at 34, so this fails
    // loudly if the left operand ever wins.
    assert_eq!(pos_of("a", "{ a = 1; } // { a = 2; }"), col(48));
}

/// The other half of the same rule, and A KNOWN DIVERGENCE: an attribute the
/// right operand does not have keeps the LEFT's value, and cppnix keeps the
/// left's position with it (column 34). One origin per set cannot express
/// that, so the answer is `null`. See `maintainers/ix/positions.md`.
#[test]
fn update_answers_null_for_an_attribute_only_the_left_had() {
    assert_eq!(pos_of("a", "{ a = 1; } // { b = 2; }"), "null");
}

/// `rec { __overrides = ...; }` is the same rule reached by a different road,
/// and it is worth pinning separately because nothing in the source says
/// `//`: the compiler closes the statics into one set and appends the
/// override set with an `Update` (`compile::emit_rec_set_build`), so the
/// result takes the OVERRIDE set's origin.
///
/// An overridden attribute therefore answers cppnix's column exactly -- 54,
/// inside `{ a = 20; }`, which is where cppnix reads it from too -- and a
/// static the override does not name answers `null` where cppnix says 72.
/// Both columns measured against `nix-instantiate --eval --strict` on the
/// wrapped fixture, 2026-08-06; the divergence is the `//`-left one in
/// `maintainers/ix/positions.md`, not a new one.
#[test]
fn a_rec_override_answers_the_override_sites_position() {
    let src = "rec { __overrides = { a = 20; }; a = 1; b = 2; }";
    assert_eq!(pos_of("a", src), col(54));
    assert_eq!(pos_of("b", src), "null");
}

/// `removeAttrs` keeps the values it did not remove, so it keeps its own
/// origin and the survivors still answer.
#[test]
fn remove_attrs_keeps_the_surviving_positions() {
    assert_eq!(
        pos_of("a", r#"builtins.removeAttrs { a = 1; b = 2; } [ "b" ]"#),
        col(55)
    );
    assert_eq!(
        pos_of("b", r#"builtins.removeAttrs { a = 1; b = 2; } [ "b" ]"#),
        "null",
        "a removed attribute is absent, so it answers null even though its \
         name is still in the site table"
    );
}

/// `intersectAttrs` takes the second set's values, so it takes its origin.
#[test]
fn intersect_attrs_takes_the_second_sets_origin() {
    // 69 is the second set's `a`; the first set's is at 57.
    assert_eq!(
        pos_of("a", "builtins.intersectAttrs { a = 0; } { a = 1; b = 2; }"),
        col(69)
    );
}

/// `builtins.functionArgs` hands back a set whose attributes cppnix gives the
/// formals' own positions (`primops.cc`, `prim_functionArgs`).
#[test]
fn function_args_answers_the_formals_positions() {
    assert_eq!(
        at_file(
            "builtins.unsafeGetAttrPos \"b\" (builtins.functionArgs ({ a, b }: 0))",
            F,
        ),
        col(60)
    );
}

/// Every component of one binding takes the position of the whole attrpath,
/// which is what cppnix's parser hands `addAttr`. `{ a.b = 1; }` answers the
/// `a` for `b` as well as for `a`, and `{ a.b.c = 1; }` answers it for `c`.
#[test]
fn a_nested_attrpath_answers_where_the_path_starts() {
    assert_eq!(pos_of("a", "{ a.b = 1; }"), col(34));
    assert_eq!(pos_of("b", "{ a.b = 1; }.a"), col(34));
    assert_eq!(pos_of("c", "{ a.b.c = 1; }.a.b"), col(34));
}

/// An inherited attribute takes the position of the NAME in the `inherit`
/// list, not of whatever it was inherited from.
#[test]
fn an_inherited_attribute_answers_its_name_in_the_inherit_list() {
    assert_eq!(pos_of("a", "let a = 1; in { inherit a; }"), col(56));
}

#[test]
fn a_rec_set_answers_like_a_plain_one() {
    assert_eq!(pos_of("a", "rec { a = 1; }"), col(38));
}

// -- positions on errors -----------------------------------------------------

/// A failing evaluation carries the position of the op that failed.
#[test]
fn an_error_carries_a_position() {
    let pos = eval_str_at("let x = 1; in\n  x.y", "/private/tmp", Origin::File(F))
        .err()
        .and_then(|e| e.pos().cloned());
    assert!(pos.is_some(), "no position on the error");
    let Some(pos) = pos else { return };
    assert_eq!(pos.line, 2, "{pos:?}");
    assert_eq!(pos.file.as_deref(), Some(F), "{pos:?}");
}

/// A `throw` carries the position of the `throw` token.
///
/// This is the case that made the attribution apply to every frame kind and
/// not only to `Frame::Unit`: `throw` raises from inside a builtin task, so
/// attributing only unit frames left it with no position at the top level and
/// with the enclosing unit's first op inside one.
#[test]
fn a_throw_carries_a_position() {
    let pos = eval_str_at(
        "let\n  boom = throw \"no\";\nin boom",
        "/private/tmp",
        Origin::File(F),
    )
    .err()
    .and_then(|e| e.pos().cloned());
    assert!(pos.is_some(), "a throw carried no position");
    let Some(pos) = pos else { return };
    assert_eq!((pos.line, pos.column), (2, 10), "{pos:?}");
}

/// Positions on the error shapes a user meets most, each read off cppnix on
/// the same file. The tuple is `(line, column)`.
#[test]
fn the_common_error_shapes_carry_cppnixs_position() {
    for (src, want) in [
        ("\n\n  throw \"no\"", (3, 3)),
        // The argument is an expression, so a unit runs between the call op
        // and the builtin task; the position must still be the `throw`.
        ("\n\n  throw (\"a\" + \"b\")", (3, 3)),
        ("\n\n  builtins.head [ (throw \"no\") ]", (3, 20)),
        ("\n\nlet f = _:\n  throw \"no\";\nin f 1", (4, 3)),
        ("\n\n  abort \"no\"", (3, 3)),
        ("\n\n  1 / 0", (3, 5)),
        ("\n\n  ({ a = 1; }).zz", (3, 3)),
        ("let\n  a = 1;\nin a + \"s\"", (3, 8)),
        ("let\n  a = 1;\nin assert a == 2; a", (3, 4)),
    ] {
        let pos = eval_str_at(src, "/private/tmp", Origin::File(F))
            .err()
            .and_then(|e| e.pos().cloned());
        assert!(pos.is_some(), "no position for {src:?}");
        let Some(pos) = pos else { continue };
        assert_eq!((pos.line, pos.column), want, "for {src:?}");
    }
}
