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
//! An annotation is parsed once into [`ty::Ty`], the crate's own description
//! of it, and lowered twice: to the checker above, and to a JSON Schema
//! ([`schema`]) so `--help` text and editor completion for a module's
//! parameters are generated rather than hand-mirrored.
//!
//! Type spellings: `string`, `bool`, `int`, `float`, `drv`, `any`/`unknown`,
//! `object`, `T[]`, `Record<string, T>`, `{ a: T; b?: U }`, function types
//! (callability only), literal unions, and `T \| null`, plus refinements:
//! Rust-width integers `u8`/`u16`/`u32`/`i8`/`i16`/`i32` and the nixpkgs
//! `lib.types` basics `port`, `path`, `nonEmptyStr`. `number` is a hard error (Nix splits int and float),
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

// Private: nothing here is part of the public API, so a `pub mod` would
// publish a module whose every item is crate-visible. Items inside are `pub`
// rather than `pub(crate)` because the private module already bounds them, and
// `redundant_pub_crate` says so.
mod checker;
pub mod error;
pub mod map;
pub mod nix;
pub mod render;
pub mod schema;
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
    Ok(render::module(&mapped(source)?.module))
}

/// The JSON Schema (draft 2020-12) for a `.ix` module's parameters.
///
/// Generated from the same annotations [`convert`] lowers to eval-time checks,
/// so the two cannot drift; see [`schema::document`] for what the document
/// describes and what it deliberately cannot.
///
/// # Errors
///
/// Returns the same positioned [`Error`] [`convert`] would: a module has a
/// schema exactly when it converts.
pub fn schema(source: &str) -> Result<String, Error> {
    Ok(schema::document(&mapped(source)?.types))
}

/// A `.ix` module's types, for a consumer rendering something other than JSON
/// Schema (`--help` text, ambient `.d.ts` declarations).
///
/// # Errors
///
/// Returns the same positioned [`Error`] [`convert`] would.
pub fn types(source: &str) -> Result<ty::ModuleTypes, Error> {
    Ok(mapped(source)?.types)
}

/// Parses `.ix` source and runs the single mapping pass over it.
fn mapped(source: &str) -> Result<map::Mapped, Error> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::ts()).parse();

    if let Some(diagnostic) = parsed.diagnostics.as_slice().first() {
        let offset = diagnostic
            .labels
            .first()
            .map_or(0, oxc_diagnostics::LabeledSpan::offset);
        let mut message = format!("parse error: {}", diagnostic.message);
        if looks_like_nix(source) {
            message.push_str(NIX_SOURCE_HINT);
        }
        return Err(Error::at_offset32(offset, source, message));
    }

    map::module(&parsed.program, source)
}

/// Appended to a parse error when [`looks_like_nix`] fires. One string, so
/// every surface the diagnostic reaches (the `builtins.wasm` trap inside an
/// eval VM, a CLI running the converter natively) carries the same words.
const NIX_SOURCE_HINT: &str = "\nhelp: this file looks like Nix source, and `.ix` modules use \
                               JavaScript syntax. Nix source belongs in a `.nix` file: a flake is \
                               `flake.nix`, an ix config module is `ix.nix`.";

/// Whether source that failed to parse as JavaScript reads as Nix instead.
///
/// The mistake this names is real and external: a shared Nix flake saved as
/// `default.ix`, where the JS parser dies at the first hyphenated attribute
/// name (`extra-substituters`) and the raw diagnostic surfaces as a wasm
/// backtrace that reads like a converter crash, not like the user's own file.
/// Writing Nix into `.ix` is the most likely recurring failure of a dialect
/// whose whole point is *not* being Nix, so it gets a purpose-built message.
///
/// Deliberately conservative, and only consulted after the parse has already
/// failed: the source must open like a Nix attrset, function, or comment
/// (`{` or `#`, but not a JS hashbang) AND carry Nix's `name = value;`
/// binding shape closed by `};`/`];`/`";` -- the shape a flake or NixOS
/// module cannot avoid, and idiomatic `.ix` (an `export default` module)
/// cannot have.
fn looks_like_nix(source: &str) -> bool {
    let trimmed = source.trim_start();
    let opens_like_nix =
        trimmed.starts_with('{') || (trimmed.starts_with('#') && !trimmed.starts_with("#!"));
    let has_nix_binding = source.contains(" = ")
        && ["};", "];", "\";"]
            .iter()
            .any(|close| source.contains(close));
    opens_like_nix && has_nix_binding
}
