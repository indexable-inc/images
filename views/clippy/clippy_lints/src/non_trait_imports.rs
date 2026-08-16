use clippy_utils::diagnostics::span_lint_and_sugg;
use rustc_errors::Applicability;
use rustc_hir::def::{DefKind, Res};
use rustc_hir::{Item, ItemKind, UseKind};
use rustc_lint::{LateContext, LateLintPass};
use rustc_middle::ty::Visibility;
use rustc_session::declare_lint_pass;
use rustc_span::symbol::kw;

declare_clippy_lint! {
    /// ### What it does
    /// Checks for `use` statements that import non-trait items.
    ///
    /// ### Why restrict this?
    /// Prefer fully qualified paths at call sites (`addr::Addr`, `vm::Config`).
    /// Only traits need `use` imports (as `_`) for method resolution.
    /// Owner-type re-exports (`pub use addr::Addr`) are allowed when the
    /// imported name matches the parent module name.
    ///
    /// ### Example
    /// ```rust,ignore
    /// use std::collections::HashMap;
    ///
    /// let m: HashMap<_, _> = HashMap::new();
    /// ```
    ///
    /// Use instead:
    /// ```rust,ignore
    /// let m: std::collections::HashMap<_, _> = std::collections::HashMap::new();
    /// ```
    #[clippy::version = "1.86.0"]
    pub NON_TRAIT_IMPORTS,
    restriction,
    "`use` items that import non-trait, non-owner-reexport items"
}

declare_lint_pass!(NonTraitImports => [NON_TRAIT_IMPORTS]);

impl<'tcx> LateLintPass<'tcx> for NonTraitImports {
    fn check_item(&mut self, cx: &LateContext<'tcx>, item: &'tcx Item<'tcx>) {
        if item.span.from_expansion() {
            return;
        }

        let ItemKind::Use(path, UseKind::Single(ident)) = item.kind else {
            return;
        };

        // Allow trait imports (handled by unused_trait_names for the `as _` style)
        if matches!(
            path.res.type_ns,
            Some(Res::Def(DefKind::Trait | DefKind::TraitAlias, _))
        ) {
            return;
        }

        // Allow underscore imports
        if ident.name == kw::Underscore {
            return;
        }

        // Allow pub owner-type re-exports: `pub use addr::Addr`
        if is_pub_owner_reexport(cx, item, path) {
            return;
        }

        let qualified = path
            .segments
            .iter()
            .map(|s| s.ident.as_str())
            .collect::<Vec<_>>()
            .join("::");

        span_lint_and_sugg(
            cx,
            NON_TRAIT_IMPORTS,
            item.span,
            "non-trait `use` import",
            format!("remove and use `{qualified}` at call sites"),
            String::new(),
            Applicability::MachineApplicable,
        );
    }
}

fn is_pub_owner_reexport<'tcx>(
    cx: &LateContext<'tcx>,
    item: &Item<'tcx>,
    path: &rustc_hir::UsePath<'tcx>,
) -> bool {
    // Must be visible beyond the parent module (pub, pub(crate), pub(super), etc.)
    let module = cx.tcx.parent_module_from_def_id(item.owner_id.def_id);
    if cx.tcx.visibility(item.owner_id.def_id) == Visibility::Restricted(module.to_def_id()) {
        return false;
    }

    let segments = &path.segments;
    if segments.len() < 2 {
        return false;
    }

    let parent_module = segments[segments.len() - 2].ident.as_str();
    let imported_name = segments[segments.len() - 1].ident.as_str();

    names_match_owner(parent_module, imported_name)
}

/// Case-insensitive, underscore-ignoring match.
/// addr/Addr, vm_config/VmConfig, open/open all match.
fn names_match_owner(module_name: &str, type_name: &str) -> bool {
    let module_lower: String = module_name
        .chars()
        .filter(|c| *c != '_')
        .flat_map(|c| c.to_lowercase())
        .collect();
    let type_lower: String = type_name
        .chars()
        .flat_map(|c| c.to_lowercase())
        .collect();
    module_lower == type_lower
}
