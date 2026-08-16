use clippy_utils::diagnostics::span_lint_and_then;
use rustc_errors::Applicability;
use rustc_hir::{Item, ItemKind};
use rustc_lint::{LateContext, LateLintPass};
use rustc_session::impl_lint_pass;
use rustc_span::Span;
use rustc_span::def_id::LOCAL_CRATE;

#[derive(Copy, Clone)]
enum NameKind {
    Function,
    Type,
    Constant,
    Other,
}

declare_clippy_lint! {
    /// ### What it does
    /// Checks that item names do not repeat `_`-separated words already
    /// present in the crate name or any ancestor module name.
    ///
    /// ### Why restrict this?
    /// `guest::spawn_vm_guest()` can be `guest::spawn()` — the repeated
    /// words add noise without information. The module path already
    /// communicates the domain.
    ///
    /// After renaming, prefer inline qualified paths (`module::Item`) over
    /// `use` imports or `pub use` re-exports. Re-exports hide the canonical
    /// owner; inline paths make the module context visible at every call
    /// site and eliminate stale re-export chains.
    ///
    /// ### Example
    /// ```rust,ignore
    /// mod guest {
    ///     pub fn spawn_vm_guest() { .. }
    /// }
    /// // caller:
    /// use crate::guest::spawn_vm_guest;
    /// spawn_vm_guest();
    /// ```
    /// Use instead:
    /// ```rust,ignore
    /// mod guest {
    ///     pub fn spawn() { .. }
    /// }
    /// // caller — inline qualified path, no `use`:
    /// guest::spawn();
    /// ```
    #[clippy::version = "1.86.0"]
    pub PATH_SEGMENT_REPETITION,
    restriction,
    "item name repeats words from its module path"
}

pub struct PathSegmentRepetition {
    /// Flattened `_`-split words from all ancestor scopes. Reuses capacity.
    words: Vec<String>,
    /// Boundary indices into `words` — one per scope push.
    boundaries: Vec<usize>,
}

impl PathSegmentRepetition {
    pub fn new() -> Self {
        Self {
            words: Vec::new(),
            boundaries: Vec::new(),
        }
    }

    fn push_scope(&mut self, name: &str) {
        self.boundaries.push(self.words.len());
        for segment in name.split('_') {
            if segment.len() >= 2 {
                self.words.push(segment.to_ascii_lowercase());
            }
        }
    }

    fn pop_scope(&mut self) {
        if let Some(boundary) = self.boundaries.pop() {
            self.words.truncate(boundary);
        }
    }

    fn is_path_word(&self, word: &str) -> bool {
        let lower = word.to_ascii_lowercase();
        self.words.iter().any(|pw| same_stem(&lower, pw))
    }

    fn check_name(&self, cx: &LateContext<'_>, name: &str, span: Span, kind: NameKind) {
        if self.words.is_empty() {
            return;
        }

        // Allow exact grouping matches: `vm::Vm`, `config::Config`,
        // `flakeref::FlakeRef`, `typeid::TypeId`. The item name (lowercased,
        // underscores stripped) exactly equals one ancestor scope word — the
        // type IS the module, not a redundant prefix.
        let joined: String = name.to_ascii_lowercase().replace('_', "");
        if self.words.iter().any(|pw| *pw == joined) {
            return;
        }

        let is_snake = name.contains('_');
        let parts: Vec<&str> = if is_snake {
            name.split('_').collect()
        } else {
            split_camel(name)
        };

        let mut repeated = Vec::new();
        let mut kept = Vec::new();
        for part in &parts {
            if part.len() >= 2 && self.is_path_word(part) {
                repeated.push(*part);
            } else {
                kept.push(*part);
            }
        }

        if repeated.is_empty() {
            return;
        }

        repeated.sort_unstable();
        repeated.dedup();

        span_lint_and_then(
            cx,
            PATH_SEGMENT_REPETITION,
            span,
            format!("item name repeats path words: {}", repeated.join(", ")),
            |diag| {
                if kept.is_empty() {
                    let hint = match kind {
                        NameKind::Function => {
                            "every word repeats the module path — rename to describe \
                             the operation, e.g. `run`, `execute`, `apply`, `process`, `new`"
                        },
                        NameKind::Type => {
                            "every word repeats the module path — rename to describe \
                             what this type represents, e.g. `Instance`, `Inner`, `Output`, `Config`"
                        },
                        NameKind::Constant => {
                            "every word repeats the module path — rename to describe \
                             what this constant represents"
                        },
                        NameKind::Other => {
                            "every word repeats the module path — choose a name that \
                             adds information beyond the module context"
                        },
                    };
                    diag.help(hint);
                } else {
                    let suggested = if is_snake {
                        kept.join("_")
                    } else {
                        kept.concat()
                    };
                    diag.span_suggestion(
                        span,
                        "remove words already expressed by the module path",
                        suggested,
                        Applicability::MachineApplicable,
                    );
                }
                diag.note(
                    "after renaming, use inline qualified paths (e.g. `module::Item`) \
                     at call sites instead of `use` imports or `pub use` re-exports",
                );
            },
        );
    }
}

/// Check if two lowercase words share a morphological stem.
///
/// Handles common English derivational suffixes found in code identifiers:
/// `-er`, `-or`, `-ing`, `-ed`, `-ment`, `-s`, `-tion`, `-ation`, `-able`,
/// `-ible`, `-ive`, `-ness`, `-ful`, `-ly`.
///
/// Also handles silent-e reinsertion (`write` → `writ`+`er`) and
/// doubled-consonant reduction (`run` → `runn`+`er`).
fn same_stem(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    if short.len() < 2 {
        return false;
    }

    // Ordered longest-first so "ation" is tried before "tion", etc.
    const SUFFIXES: &[&str] = &[
        "ation", "tion", "ment", "able", "ible", "ness", "ing", "ful", "ive", "er", "or", "ed",
        "ly", "s",
    ];

    for suffix in SUFFIXES {
        let Some(base) = long.strip_suffix(suffix) else {
            continue;
        };
        if base.len() < 2 {
            continue;
        }
        // Exact: build == build (from builder)
        if base == short {
            return true;
        }
        // Silent-e: write → writ+er, parse → pars+er
        if short.strip_suffix('e').is_some_and(|s| s == base) {
            return true;
        }
        // Doubled consonant: run → runn+er, map → mapp+er
        let bytes = base.as_bytes();
        let last = bytes[bytes.len() - 1];
        if last == bytes[bytes.len() - 2] && last.is_ascii_lowercase() && &base[..base.len() - 1] == short {
            return true;
        }
        // -ation: create → cre+ation (base + "ate" == short)
        if *suffix == "ation"
            && short.len() == base.len() + 3
            && short.starts_with(base)
            && short.ends_with("ate")
        {
            return true;
        }
        // -tion: connect → connec+tion (short minus trailing 't' == base)
        if *suffix == "tion" && short.strip_suffix('t').is_some_and(|s| s == base) {
            return true;
        }
    }
    false
}

/// Split a CamelCase name into its word parts.
///
/// Handles acronyms: `HTTPServer` → `["HTTP", "Server"]`,
/// `ForkMemory` → `["Fork", "Memory"]`.
fn split_camel(name: &str) -> Vec<&str> {
    let bytes = name.as_bytes();
    let mut parts = Vec::new();
    let mut start = 0;
    for i in 1..bytes.len() {
        let cur_upper = bytes[i].is_ascii_uppercase();
        if !cur_upper {
            continue;
        }
        let prev_lower = bytes[i - 1].is_ascii_lowercase();
        let prev_upper = bytes[i - 1].is_ascii_uppercase();
        let next_lower =
            i + 1 < bytes.len() && bytes[i + 1].is_ascii_lowercase();
        // Break before: uppercase after lowercase (`forkM`)
        // or acronym end before new word (`HTTPs` → `HTTP`, `Server`)
        if prev_lower || (prev_upper && next_lower) {
            parts.push(&name[start..i]);
            start = i;
        }
    }
    parts.push(&name[start..]);
    parts
}

impl_lint_pass!(PathSegmentRepetition => [PATH_SEGMENT_REPETITION]);

impl<'tcx> LateLintPass<'tcx> for PathSegmentRepetition {
    fn check_crate(&mut self, cx: &LateContext<'tcx>) {
        let crate_name = cx.tcx.crate_name(LOCAL_CRATE);
        self.push_scope(crate_name.as_str());
    }

    fn check_crate_post(&mut self, _: &LateContext<'tcx>) {
        self.pop_scope();
    }

    fn check_item(&mut self, cx: &LateContext<'tcx>, item: &'tcx Item<'tcx>) {
        if let ItemKind::Mod(ident, _) = &item.kind {
            self.push_scope(ident.name.as_str());
            return;
        }

        let Some(ident) = item.kind.ident() else {
            return;
        };

        if ident.span.from_expansion() {
            return;
        }

        if matches!(item.kind, ItemKind::Use(..) | ItemKind::ExternCrate(..)) {
            return;
        }

        let kind = match &item.kind {
            ItemKind::Fn { .. } => NameKind::Function,
            ItemKind::Struct(..)
            | ItemKind::Enum(..)
            | ItemKind::Union(..)
            | ItemKind::Trait { .. }
            | ItemKind::TraitAlias(..)
            | ItemKind::TyAlias(..) => NameKind::Type,
            ItemKind::Const(..) | ItemKind::Static(..) => NameKind::Constant,
            _ => NameKind::Other,
        };
        self.check_name(cx, ident.name.as_str(), ident.span, kind);
    }

    fn check_item_post(&mut self, _: &LateContext<'tcx>, item: &'tcx Item<'tcx>) {
        if matches!(item.kind, ItemKind::Mod(..)) {
            self.pop_scope();
        }
    }

    // Impl items and trait items are skipped: method/type/const names inside
    // `impl` blocks or `trait` definitions are either dictated by a trait
    // contract (cannot be renamed) or scoped to the impl (not part of the
    // module's public API surface where repetition matters).
}

#[cfg(test)]
mod tests {
    use super::same_stem;

    #[test]
    fn exact() {
        assert!(same_stem("write", "write"));
        assert!(same_stem("build", "build"));
    }

    #[test]
    fn suffix_er() {
        assert!(same_stem("write", "writer"));
        assert!(same_stem("writer", "write"));
        assert!(same_stem("build", "builder"));
        assert!(same_stem("parse", "parser"));
        assert!(same_stem("compile", "compiler"));
        assert!(same_stem("handle", "handler"));
        assert!(same_stem("resolve", "resolver"));
        assert!(same_stem("serve", "server"));
    }

    #[test]
    fn suffix_or() {
        assert!(same_stem("create", "creator"));
        assert!(same_stem("execute", "executor"));
        assert!(same_stem("allocate", "allocator"));
        assert!(same_stem("iterate", "iterator"));
    }

    #[test]
    fn suffix_ing() {
        assert!(same_stem("build", "building"));
        assert!(same_stem("write", "writing"));
        assert!(same_stem("spawn", "spawning"));
    }

    #[test]
    fn suffix_ed() {
        assert!(same_stem("parse", "parsed"));
        assert!(same_stem("compile", "compiled"));
        assert!(same_stem("cache", "cached"));
    }

    #[test]
    fn suffix_ment() {
        assert!(same_stem("deploy", "deployment"));
        assert!(same_stem("assign", "assignment"));
    }

    #[test]
    fn suffix_tion() {
        assert!(same_stem("connect", "connection"));
        assert!(same_stem("create", "creation"));
        assert!(same_stem("compile", "compilation"));
        assert!(same_stem("configure", "configuration"));
    }

    #[test]
    fn suffix_able() {
        assert!(same_stem("write", "writable"));
        assert!(same_stem("read", "readable"));
        assert!(same_stem("configure", "configurable"));
    }

    #[test]
    fn suffix_s() {
        assert!(same_stem("event", "events"));
        assert!(same_stem("node", "nodes"));
    }

    #[test]
    fn doubled_consonant() {
        assert!(same_stem("run", "runner"));
        assert!(same_stem("map", "mapper"));
        assert!(same_stem("wrap", "wrapper"));
        assert!(same_stem("scan", "scanner"));
    }

    #[test]
    fn no_false_positives() {
        assert!(!same_stem("file", "filter"));
        assert!(!same_stem("read", "render"));
        assert!(!same_stem("net", "network"));
        assert!(!same_stem("log", "logic"));
        assert!(!same_stem("port", "portal"));
    }
}
