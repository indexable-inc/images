//! One `builtins` name set per process, and one spelling per set.
//!
//! `set_cpp_builtin_names` is set-once for the same reason the store
//! directory is (ENG-12541): two sets disagree about which names exist, so a
//! result memoised under the first would be served under the second. Order
//! and repeats are not a second set, though, and refusing those would make
//! the bridge's ordinary repeat call an error.
//!
//! Its own binary because the lock moves once per process.

use nix_eval_rs::eval;

#[test]
fn the_name_set_is_canonicalised() {
    assert!(eval::set_cpp_builtin_names("fetchTree  abort").is_ok());
    assert!(
        eval::set_cpp_builtin_names("abort fetchTree abort").is_ok(),
        "the same set spelled differently was refused as a conflict"
    );
    assert!(
        eval::set_cpp_builtin_names("abort").is_err(),
        "a genuinely different name set was accepted, so a memoised result \
         computed under the first is now addressable under the second"
    );
}
