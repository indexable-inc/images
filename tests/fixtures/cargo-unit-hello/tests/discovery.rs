//! Integration-test target for the test-discovery parity fixtures.
//!
//! Exists so the workspace has a second non-empty test target (beside the
//! lib unittests) whose discovered name set the fork-toolchain parity check
//! in tests/default.nix byte-compares between the "binary" and
//! "dump-test-names" manifest modes. The shapes here are chosen for
//! discovery, not for what they compute: nested modules (name paths),
//! `#[ignore]` with and without a message (the `.ignored.list` half of the
//! manifest), `#[should_panic]` (extra descriptor fields that must not leak
//! into the name), and a macro-generated test (names the harness collects
//! only after expansion).

#[test]
fn top_level_case() {
    assert_eq!(cargo_unit_hello::greeting(), "hello from cargo-unit");
}

#[test]
#[ignore]
fn ignored_case() {
    unreachable!("never runs without --include-ignored");
}

#[test]
#[ignore = "parity fixture: ignore message must not perturb the name"]
fn ignored_with_message_case() {}

#[test]
#[should_panic(expected = "on purpose")]
fn should_panic_case() {
    panic!("on purpose");
}

mod nested {
    mod deeper {
        #[test]
        fn nested_case() {}
    }
}

macro_rules! generated_test {
    ($name:ident) => {
        #[test]
        fn $name() {}
    };
}

generated_test!(macro_generated_case);
