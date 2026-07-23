//! `ix2nix`: a pure source-to-source converter from `.ix` modules to Nix.
//!
//! A `.ix` file is TypeScript *syntax* used as a 1:1 skin over Nix
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
//! # Types
//!
//! TypeScript annotations lower to *runtime checks* against the `__ixTy`
//! runtime the importer passes (`ix-ty.nix`; `assert` mode checks, `erase`
//! mode no-ops). Static checking is tsc's job, never this converter's.
//!
//! | `.ix` (TypeScript syntax) | emitted Nix |
//! |---|---|
//! | `(a: T) => e` | `a: __ixTy.arg "<line>:<col> ..." <T> a e` |
//! | `(a): R => e` | `a: __ixTy.ret "<line>:<col> return" <R> e` |
//! | `e as T` | `__ixTy.ret "<line>:<col> as" <T> e` (`as any`/`as unknown`: nothing) |
//! | `type X = T` | `let ty'X = <T>` (referenced by annotations) |
//! | `({ a }: { a: T }) => e` | per-field `__ixTy.arg` checks on the bound names |
//!
//! Type spellings: `string`, `bool`, `Int`, `Float`, `Drv`, `any`/`unknown`,
//! `object`, `T[]`, `Record<string, T>`, `{ a: T; b?: U }`, function types
//! (callability only), literal unions, and `T \| null`, plus refinements
//! borrowed from nixpkgs `lib.types` basics: `Uint`, `Port`, `Path`,
//! `NonEmptyString`. `number` is a hard error (Nix splits int and float),
//! as are interfaces, generics, `satisfies`, non-null `!`, and unions
//! beyond `T \| null` / literals.
//!
//! Every module renders wrapped as `{ __dir, __importIx, __ixTy }: <body>`
//! -- one calling convention for importers, whether or not the source used
//! `import()` or type annotations.
//!
//! Deliberate hard errors (no 1:1): `===`/`!==` (use `==`/`!=`), `undefined`
//! (use `null`), bare `??` or bare `?.`, `let`/`var`, mutation, loops and
//! statements, `function` expressions, zero-argument functions and calls,
//! numeric indexing (use `builtins.elemAt`), `%`, `**`, bitwise operators.

pub mod error;
pub mod map;
pub mod nix;
pub mod render;
pub mod ty;

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
    let parsed = Parser::new(&allocator, source, SourceType::ts()).parse();

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
