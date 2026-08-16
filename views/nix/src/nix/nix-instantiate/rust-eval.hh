#pragma once
///@file The nix-instantiate seam into the Rust evaluator (rust/nix-eval-rs).
/// M1 scope: whole-expression evaluation of source text. The declaration is
/// unconditional; without -Drust-eval the definition throws, so the setting
/// still parses and reports a clear error on use.

#include "nix/expr/eval.hh"

namespace nix {

/// Evaluate source text with the Rust backend and print the result the way
/// processExpr would. Throws EvalError on evaluation failure and Error with
/// the marker "rust-eval unimplemented" on the shapes the backend (or this
/// bridge) does not cover: lazy top-level printing, and `--xml` with source
/// locations (`xmlLocation`, nix-instantiate's `xmlOutputSourceLocation` --
/// the Rust document has no position attributes, so only the `--no-location`
/// spelling is served).
/// `file` is the absolute path `source` was read from, or empty when it came
/// from `--expr`. It is what `__curPos` reports (ENG-12713).
void rustEvalPrint(
    EvalState & state,
    const std::string & source,
    const std::string & baseDir,
    const std::string & file,
    const Strings & attrPaths,
    int outputKind,
    bool xmlLocation,
    bool strict);

} // namespace nix
