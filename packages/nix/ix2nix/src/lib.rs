//! `ix2nix`: a pure source-to-source converter from `.ix` modules to Nix.
//!
//! A `.ix` file is `JavaScript` *syntax* used as a 1:1 skin over Nix
//! *semantics*: no `JavaScript` runtime behavior, no evaluation. [`convert`]
//! takes `.ix` source in and returns Nix source out. Anything without exactly
//! one Nix spelling is a positioned [`Error`] — there are no fallbacks.
//!
//! # The 1:1 mapping
//!
//! | `.ix` (`JavaScript` syntax) | Nix |
//! |---|---|
//! | `export default expr` (after top-level `const`s) | `let ... in expr` |
//! | `(a) => expr` | `a: expr` |
//! | `(a, b) => expr` | `a: b: expr` |
//! | `({ a, b = 1, ...rest }) => expr` | `{ a, b ? 1, ... } @ rest: expr` |
//! | arrow block of `const`s + `return expr` | `let ... in expr` |
//! | `{ a: 1, "b c": 2, [k]: 3 }` | `{ a = 1; "b c" = 2; ${k} = 3; }` |
//! | `{ ...a, b: 1 }` | `a // { b = 1; }` |
//! | `[1, 2]` / `[...a, b]` | `[ 1 2 ]` / `a ++ [ b ]` |
//! | `` `x${e}` `` | `"x${e}"` |
//! | `x.y`, `x["y z"]`, `x[k]` | `x.y`, `x."y z"`, `x.${k}` |
//! | `x.y?.z ?? d` | `x.y.z or d` |
//! | `c ? a : b` | `if c then a else b` |
//! | `f(a, b)` | `f a b` |
//! | `import("./x.ix")` | `__importIx (__dir + "/x.ix")` |
//! | `import("./x.nix")` | `import (__dir + "/x.nix")` |
//! | `+ - * / == != < <= > >= && \|\| !` | the same operators |
//! | `true` / `false` / `null` | `true` / `false` / `null` |
//!
//! A module whose source uses `import()` renders wrapped as
//! `{ __dir, __importIx }: <body>`; otherwise the body stands alone.
//!
//! Deliberate hard errors (no 1:1): `===`/`!==` (use `==`/`!=`), `undefined`
//! (use `null`), bare `??` or bare `?.`, `let`/`var`, mutation, loops and
//! statements, `function` expressions, zero-argument functions and calls,
//! numeric indexing (use `builtins.elemAt`), `%`, `**`, bitwise operators.

pub mod error;
pub mod map;
pub mod nix;
pub mod render;

pub use error::Error;

use oxc_allocator::Allocator;
use oxc_parser::Parser;
use oxc_span::SourceType;

/// Converts `.ix` source text to Nix source text.
///
/// # Errors
///
/// Returns a positioned [`Error`] when the source is not a valid ES module or
/// uses any `JavaScript` form without a 1:1 Nix equivalent.
pub fn convert(source: &str) -> Result<String, Error> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::mjs()).parse();

    if let Some(diagnostic) = parsed.diagnostics.as_slice().first() {
        let offset = diagnostic
            .labels
            .first()
            .map_or(0, oxc_diagnostics::LabeledSpan::offset);
        return Err(Error::at_offset32(
            offset,
            source,
            format!("parse error: {}", diagnostic.message),
        ));
    }

    let module = map::module(&parsed.program, source)?;
    Ok(render::module(&module))
}
