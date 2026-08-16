//! When cppnix says it has the gated names, so does this backend.
//!
//! ENG-12717's other half. `builtins_gating_default.rs` shows the names are
//! absent when the embedder has said nothing; on its own that is also what a
//! backend that had simply deleted them would show. This binary hands over a
//! name set that contains them -- the shape `ixe_set_cpp_builtin_names`
//! carries from `EvalState::getBuiltins()` -- and requires each one to
//! reappear, so the gate is a gate and not a removal.
//!
//! A separate binary because the name set is set-once per process.

use nix_eval_rs::eval;

fn has(name: &str) -> bool {
    matches!(eval::eval_str(&format!("builtins ? {name}")), Ok(answer) if answer == "true")
}

#[test]
fn a_name_cppnix_registered_is_advertised_here() {
    // What cppnix's `builtins` holds with every gate open: the stripped
    // spellings, which is what the attrset keys are.
    assert!(
        eval::set_cpp_builtin_names(
            "exec fetchClosure fetchTree importNative outputOf parallel wasm stringLength",
        )
        .is_ok()
    );

    for name in [
        "fetchClosure",
        "outputOf",
        "parallel",
        "wasm",
        "fetchTree",
        "exec",
        "importNative",
    ] {
        assert!(
            has(name),
            "builtins ? {name} is false where cppnix says it has it"
        );
    }
    // Not in the list cppnix sent, so not here either. These two are the
    // names cppnix never puts in the set under any setting: `fetchFinalTree`
    // is `.internal = true` and `recordedTreeAttr` is not a `RegisterPrimOp`
    // at all, which is why the generator's grep saw them and cppnix's
    // `builtins` does not.
    for name in ["fetchFinalTree", "recordedTreeAttr"] {
        assert!(
            !has(name),
            "builtins ? {name} is true; cppnix did not send it"
        );
    }
    // A short list does not delete an ungated builtin: only the gated names
    // are read from it, so `toFile` -- absent from the list above -- stays.
    assert!(
        has("toFile"),
        "an ungated builtin was deleted by the embedder's list"
    );
    // And the bare global follows the set, because `addPrimOp` fills both.
    let forced = format!("{:?}", eval::eval_str("__fetchClosure"));
    assert!(
        !forced.contains("undefined variable"),
        "__fetchClosure is still undefined where cppnix registered it: {forced}"
    );
}
