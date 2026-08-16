//! Split an exported surface over several files.
//!
//! `#[unibind::export]` lowers one module's token stream, which for a long
//! time meant one file: an attribute proc macro on `mod machines;` receives
//! no body, and `include!` hands the macro the call rather than the items.
//! So the ix SDK surface grew to 6,600 lines in a single file whose own
//! convention was to append rather than edit, because a module boundary was
//! not available (ENG-12397).
//!
//! The export names its other files instead:
//!
//! ```ignore
//! #[unibind::export(parts = ["src/sdk/machines.rs", "src/sdk/snapshots.rs"])]
//! mod _ix_sdk {
//!     use ...;
//! }
//! ```
//!
//! Each listed file is a list of items -- the same items that would have
//! been written inline -- read here and appended to the module before
//! lowering, so one lowering pass still sees the whole surface. That
//! matters: a type reference in a signature is classified against every
//! declaration in the export, so no per-file pass could resolve
//! `MachineInfo` without the file that declares it.
//!
//! Three properties the list buys, each of them a compile error when
//! broken:
//!
//! - **Order is explicit.** The combined declaration order is the module's
//!   own items followed by the parts in listed order. Nothing depends on
//!   filesystem order or on the order macros happen to expand in, and the
//!   generated layout (which mirrors declaration order) is therefore
//!   something the crate author writes down once.
//! - **A part listed twice is refused**, naming it, because its items would
//!   otherwise lower twice.
//! - **A file that sits beside the parts but is not listed is refused**,
//!   naming it. Adding a file and forgetting to register it is the failure
//!   this catches; it would otherwise be silently absent from the SDK.
//!
//! The cost, stated plainly: the items are read here rather than by rustc,
//! so a type error inside a part is reported against the `#[unibind::export]`
//! attribute rather than against the part's own line. Stable Rust gives a
//! proc macro no way to manufacture a span into a file it read. Diagnostics
//! from unibind itself name the item, and rustc still prints the offending
//! code, so the error text is intact -- the file and line are what is lost.

use std::path::{Path, PathBuf};

use proc_macro2::{Span, TokenStream};
use quote::quote;
use unibind_core::{LowerError, PartPath};

/// Append every listed part's items to `module`, in list order.
///
/// Returns the tokens that register each part as a build input, so a change
/// to a part re-expands the macro.
pub fn splice(module: &mut syn::ItemMod, parts: &[PartPath]) -> Result<TokenStream, LowerError> {
    if parts.is_empty() {
        return Ok(TokenStream::new());
    }
    // Canonicalized so the directory scan below compares like with like: the
    // listed paths are canonicalized as they resolve, and a build tree reached
    // through a symlink would otherwise never match its own contents.
    let manifest = manifest_dir()?;
    let manifest = manifest.canonicalize().unwrap_or(manifest);
    let mut resolved = Vec::new();
    for part in parts {
        let path = manifest.join(&part.path);
        let canonical = path.canonicalize().map_err(|error| {
            LowerError {
                span: part.span,
                message: format!(
                    "`{}` is listed in `parts` but {} cannot be read: {error}",
                    part.path,
                    path.display()
                ),
            }
        })?;
        if !canonical.is_file() {
            return Err(LowerError {
                span: part.span,
                message: format!(
                    "`{}` is listed in `parts` but is not a file",
                    part.path
                ),
            });
        }
        resolved.push(Resolved {
            part,
            path: canonical,
        });
    }
    reject_unlisted(&resolved, &manifest)?;
    let Some((_, items)) = &mut module.content else {
        return Err(LowerError {
            span: Span::call_site(),
            message: "#[unibind::export] needs an inline module (`mod name { ... }`); \
                      `parts = [...]` splits the module's body over files, and the \
                      module itself still opens a brace"
                .to_owned(),
        });
    };
    let mut inputs = TokenStream::new();
    for resolved in &resolved {
        items.extend(resolved.items()?);
        // Registers the part as a build input: rustc records what
        // `include_bytes!` reads, so editing a part re-runs this expansion
        // instead of leaving the stale IR compiled in.
        let path = resolved.path.to_string_lossy();
        let path = path.as_ref();
        inputs.extend(quote! {
            const _: &[u8] = include_bytes!(#path);
        });
    }
    Ok(inputs)
}

/// One listed part, resolved to a file that exists.
struct Resolved<'a> {
    part: &'a PartPath,
    path: PathBuf,
}

impl Resolved<'_> {
    /// The part's items, parsed.
    fn items(&self) -> Result<Vec<syn::Item>, LowerError> {
        let text = std::fs::read_to_string(&self.path).map_err(|error| LowerError {
            span: self.part.span,
            message: format!("reading part `{}` failed: {error}", self.part.path),
        })?;
        let file = syn::parse_file(&text).map_err(|error| LowerError {
            span: self.part.span,
            message: format!(
                "part `{}` does not parse as a list of Rust items: {error}",
                self.part.path
            ),
        })?;
        // A `//!` header would describe the file while the module's own doc
        // comment describes the export, and only one of the two can reach the
        // IR. Refusing says so rather than dropping it silently; `//` comments
        // and the banner convention are unaffected.
        if let Some(attribute) = file.attrs.first() {
            return Err(LowerError {
                span: self.part.span,
                message: format!(
                    "part `{}` opens with an inner attribute ({}), and a part is \
                     a list of items spliced into the exported module: the \
                     module's own docs and attributes belong on the \
                     #[unibind::export] module. Use a `//` comment for a file \
                     header.",
                    self.part.path,
                    quote!(#attribute)
                ),
            });
        }
        Ok(file.items)
    }
}

/// Refuse a `.rs` file that sits in a parts directory without being listed.
///
/// The list is the declaration order of the surface, so a file nobody listed
/// has no position in it -- and would otherwise just be absent from the
/// generated SDK, which is the quietest possible failure.
fn reject_unlisted(resolved: &[Resolved<'_>], manifest: &Path) -> Result<(), LowerError> {
    for directory in directories(resolved) {
        let entries = std::fs::read_dir(&directory).map_err(|error| LowerError {
            span: Span::call_site(),
            message: format!("reading {} failed: {error}", directory.display()),
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| LowerError {
                span: Span::call_site(),
                message: format!("reading {} failed: {error}", directory.display()),
            })?;
            let path = entry.path();
            if path.extension().is_none_or(|extension| extension != "rs") {
                continue;
            }
            let path = path.canonicalize().map_err(|error| LowerError {
                span: Span::call_site(),
                message: format!("reading {} failed: {error}", path.display()),
            })?;
            if resolved.iter().any(|listed| listed.path == path) {
                continue;
            }
            return Err(LowerError {
                span: first_in(resolved, &directory),
                message: format!(
                    "`{}` sits with this export's parts but is not listed in \
                     `parts = [...]`; add it (where it sits in the list is its \
                     place in declaration order, which the generated layout \
                     mirrors), or move it out of `{}`",
                    relative(&path, manifest),
                    relative(&directory, manifest)
                ),
            });
        }
    }
    Ok(())
}

/// Every directory the listed parts live in, without repeats.
fn directories(resolved: &[Resolved<'_>]) -> Vec<PathBuf> {
    let mut directories: Vec<PathBuf> = Vec::new();
    for listed in resolved {
        let Some(parent) = listed.path.parent() else {
            continue;
        };
        if !directories.iter().any(|seen| seen == parent) {
            directories.push(parent.to_path_buf());
        }
    }
    directories
}

/// The span of the first part in `directory`, so the diagnostic about an
/// unlisted sibling points at the list it belongs in.
fn first_in(resolved: &[Resolved<'_>], directory: &Path) -> Span {
    resolved
        .iter()
        .find(|listed| listed.path.parent() == Some(directory))
        .map_or_else(Span::call_site, |listed| listed.part.span)
}

/// A path as the `parts` list spells it: relative to the crate manifest
/// when it is under it, so a diagnostic reads like the list it is about
/// rather than like a build sandbox.
fn relative(path: &Path, manifest: &Path) -> String {
    path.strip_prefix(manifest)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// The crate root the part paths resolve against.
fn manifest_dir() -> Result<PathBuf, LowerError> {
    std::env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| LowerError {
            span: Span::call_site(),
            message: "`parts = [...]` resolves each path against \
                      CARGO_MANIFEST_DIR, which this expansion does not have; \
                      every cargo-compatible driver sets it"
                .to_owned(),
        })
}
