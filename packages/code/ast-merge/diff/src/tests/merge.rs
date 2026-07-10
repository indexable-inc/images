use super::rust;

struct Case {
    name: &'static str,
    base: &'static str,
    left: &'static str,
    right: &'static str,
    expected: &'static [&'static str],
}

impl Case {
    fn verify(self) {
        let result = rust(self.base, self.left, self.right);
        assert!(result.success, "{}: {}", self.name, result.content);
        for needle in self.expected {
            assert!(
                result.content.contains(needle),
                "{}: missing {needle:?} in {}",
                self.name,
                result.content
            );
        }
    }
}

#[test]
fn independent_changes_merge() {
    let cases = [
        Case {
            name: "left modifies, right adds",
            base: r#"fn original() { println!("original"); }"#,
            left: r#"fn original() { println!("modified by left"); }"#,
            right: r#"fn original() { println!("original"); }
                fn right_new() { println!("from right"); }"#,
            expected: &["modified by left", "fn right_new()", "from right"],
        },
        Case {
            name: "right modifies, left adds",
            base: r#"fn original() { println!("original"); }"#,
            left: r#"fn original() { println!("original"); }
                fn left_new() { println!("from left"); }"#,
            right: r#"fn original() { println!("modified by right"); }"#,
            expected: &["modified by right", "fn left_new()", "from left"],
        },
        Case {
            name: "both add functions",
            base: r#"fn original() { println!("original"); }"#,
            left: r#"fn original() { println!("original"); }
                fn left_new() { println!("from left"); }"#,
            right: r#"fn original() { println!("original"); }
                fn right_new() { println!("from right"); }"#,
            expected: &["fn left_new()", "fn right_new()"],
        },
        Case {
            name: "mixed functions",
            base: r#"fn foo() { println!("foo"); }
                fn bar() { println!("bar"); }"#,
            left: r#"fn foo() { println!("foo modified by left"); }
                fn bar() { println!("bar"); }
                fn baz() { println!("baz from left"); }"#,
            right: r#"fn foo() { println!("foo"); }
                fn bar() { println!("bar modified by right"); }
                fn qux() { println!("qux from right"); }"#,
            expected: &[
                "foo modified by left",
                "bar modified by right",
                "fn baz()",
                "fn qux()",
            ],
        },
        Case {
            name: "different lines",
            base: "fn process() { let a = 1; let b = 2; let c = 3; }",
            left: "fn process() { let a = 100; let b = 2; let c = 3; }",
            right: "fn process() { let a = 1; let b = 2; let c = 300; }",
            expected: &["let a = 100;", "let c = 300;"],
        },
    ];

    cases.into_iter().for_each(Case::verify);
}

#[test]
fn identical_revisions_are_not_duplicated() {
    let base = r#"fn original() { println!("original"); }"#;
    for changed in [base, r#"fn original() { println!("same change"); }"#] {
        let result = rust(base, changed, changed);
        assert!(result.success, "{}", result.content);
        assert!(result.content.contains("fn original()"));
        assert_eq!(result.content.matches("fn original()").count(), 1);
    }
}
