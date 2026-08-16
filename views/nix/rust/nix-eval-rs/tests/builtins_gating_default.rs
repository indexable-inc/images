//! With no experimental features enabled, the `builtins` set holds exactly
//! what cppnix's holds.
//!
//! ENG-12717. cppnix's registration loop skips a primop whose experimental
//! feature is off, so `builtins ? fetchClosure` is false and `__fetchClosure`
//! is an undefined variable. This backend advertised all eight of the gated
//! names unconditionally and then refused on force, which inverts the
//! standard capability test: `if builtins ? fetchClosure then <fast path>
//! else <fallback>` took the fast path here and the fallback under cppnix, so
//! code written defensively to survive a missing builtin was steered into the
//! one branch that cannot work.
//!
//! Its own test binary because the feature set is a `OnceLock`: a test
//! elsewhere in the suite that set it would decide this one's outcome by
//! running first. The other half, with the features on, is
//! `builtins_gating_enabled.rs`.

use nix_eval_rs::eval;

/// Every name cppnix registers behind a condition *and* this crate does not
/// implement. With no embedder there is no cppnix to disagree with, so the
/// standalone default advertises a gated name iff this crate has it -- which
/// leaves exactly these eight, the eight ENG-12717 measured.
///
/// `fetchTree` is gated in cppnix too and is deliberately not here: it is
/// implemented in this crate (ENG-12712's fetcher family), so hiding it
/// standalone would be its own wrong answer.
const GATED: &[&str] = &[
    "exec",
    "fetchClosure",
    "fetchFinalTree",
    "importNative",
    "outputOf",
    "parallel",
    "recordedTreeAttr",
    "wasm",
];

fn has(name: &str) -> bool {
    matches!(eval::eval_str(&format!("builtins ? {name}")), Ok(answer) if answer == "true")
}

#[test]
fn no_gated_builtin_is_advertised_by_default() {
    assert_eq!(
        eval::cpp_builtin_names(),
        None,
        "an embedder name set was already installed; this binary must be the \
         only thing setting it, or the assertions below test nothing"
    );
    for name in GATED {
        assert!(
            !has(name),
            "builtins ? {name} is true with no experimental features on; \
             cppnix says false, and a capability test that answers true for \
             something that then refuses is worse than the missing builtin"
        );
    }
}

/// The set is not simply empty of everything: an ungated name is still there.
/// Without this the test above passes on a `builtins` set that lost every
/// member.
#[test]
fn ungated_builtins_are_still_advertised() {
    for name in [
        "stringLength",
        "fetchurl",
        "storePath",
        "getFlake",
        "toFile",
        "fetchTree",
    ] {
        assert!(
            has(name),
            "builtins ? {name} is false, so the gate is over-reaching"
        );
    }
}

/// `addPrimOp` puts a registered primop in the global scope and in the set,
/// and a skipped one in neither, so the bare spelling has to be an undefined
/// variable and not this crate's `unimplemented` report.
#[test]
fn a_gated_global_is_an_undefined_variable() {
    for name in [
        "__fetchClosure",
        "__outputOf",
        "__parallel",
        "__wasm",
        "__exec",
    ] {
        let message = format!("{:?}", eval::eval_str(name));
        assert!(
            message.contains("undefined variable"),
            "{name} answered {message}; cppnix raises an undefined variable \
             for a primop it did not register"
        );
    }
}

/// And an ungated global still resolves, so the test above is not passing on
/// a compiler that lost every global.
#[test]
fn an_ungated_global_still_resolves() {
    assert_eq!(
        eval::eval_str("__stringLength \"abc\"").ok().as_deref(),
        Some("3")
    );
}
