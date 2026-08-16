use clippy_utils::diagnostics::span_lint_and_help;
use rustc_ast::ast::{Item, ItemKind, UseTree, UseTreeKind};
use rustc_lint::{EarlyContext, EarlyLintPass};
use rustc_session::declare_lint_pass;
use rustc_span::symbol::kw;

declare_clippy_lint! {
    /// ### What it does
    /// Checks for `use` statements that rename an import with `as`.
    ///
    /// ### Why restrict this?
    /// Renaming imports with `as` hides the original name and makes it harder
    /// to grep for usages. Using the fully qualified path or the original name
    /// keeps code searchable and explicit.
    ///
    /// ### Example
    /// ```rust,ignore
    /// use std::collections::HashMap as Map;
    ///
    /// let m: Map = Map::new();
    /// ```
    ///
    /// Use instead:
    /// ```rust,ignore
    /// use std::collections::HashMap;
    ///
    /// let m: HashMap = HashMap::new();
    /// ```
    #[clippy::version = "1.86.0"]
    pub RENAMED_IMPORTS,
    restriction,
    "`use` items that rename an import with `as`"
}

declare_lint_pass!(RenamedImports => [RENAMED_IMPORTS]);

impl EarlyLintPass for RenamedImports {
    fn check_item(&mut self, cx: &EarlyContext<'_>, item: &Item) {
        if let ItemKind::Use(ref use_tree) = item.kind {
            check_use_tree(use_tree, cx);
        }
    }
}

fn check_use_tree(use_tree: &UseTree, cx: &EarlyContext<'_>) {
    match use_tree.kind {
        UseTreeKind::Simple(Some(alias)) => {
            // `use Trait as _` is fine (anonymous import)
            if alias.name == kw::Underscore {
                return;
            }

            let original = use_tree
                .prefix
                .segments
                .last()
                .expect("use paths cannot be empty")
                .ident;

            // Only fire when the name actually changed
            if original.name != alias.name {
                span_lint_and_help(
                    cx,
                    RENAMED_IMPORTS,
                    item_span(use_tree),
                    "import renamed with `as`",
                    None,
                    "use the original name or the fully qualified path instead",
                );
            }
        },
        UseTreeKind::Simple(None) | UseTreeKind::Glob(_) => {},
        UseTreeKind::Nested { ref items, .. } => {
            for (nested, _) in items {
                check_use_tree(nested, cx);
            }
        },
    }
}

fn item_span(use_tree: &UseTree) -> rustc_span::Span {
    use_tree.span()
}
