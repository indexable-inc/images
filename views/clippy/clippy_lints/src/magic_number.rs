use clippy_utils::diagnostics::span_lint_and_help;
use clippy_utils::is_in_test;
use rustc_ast::LitKind;
use rustc_hir::def::DefKind;
use rustc_hir::{Expr, ExprKind, ItemKind, Node, TraitItemKind, ImplItemKind};
use rustc_lint::{LateContext, LateLintPass, LintContext};
use rustc_session::impl_lint_pass;

declare_clippy_lint! {
    /// ### What it does
    /// Flags numeric literals (integers and floats) that appear inline
    /// in non-test production code.
    ///
    /// ### Why restrict this?
    /// Magic numbers obscure intent. Every numeric literal should be
    /// either a named `const` (if the value is static/known at compile
    /// time) or a configurable value received via a struct field or
    /// function argument.
    ///
    /// ### Example
    /// ```rust,ignore
    /// fn retry(n: u32) {
    ///     std::thread::sleep(Duration::from_secs(30)); // what is 30?
    ///     for _ in 0..5 { /* ... */ }                  // what is 5?
    /// }
    /// ```
    /// Use instead:
    /// ```rust,ignore
    /// const RETRY_DELAY_SECS: u64 = 30;
    /// const MAX_RETRIES: u32 = 5;
    ///
    /// fn retry(config: &RetryConfig) {
    ///     std::thread::sleep(Duration::from_secs(RETRY_DELAY_SECS));
    ///     for _ in 0..config.max_retries { /* ... */ }
    /// }
    /// ```
    #[clippy::version = "1.86.0"]
    pub MAGIC_NUMBER,
    restriction,
    "numeric literals should be named constants or configurable values passed as arguments"
}

pub struct MagicNumber {
    allowed_ints: Vec<i64>,
    allowed_floats: Vec<String>,
}

impl MagicNumber {
    pub fn new(allowed_ints: Vec<i64>, allowed_floats: Vec<String>) -> Self {
        Self {
            allowed_ints,
            allowed_floats,
        }
    }
}

impl_lint_pass!(MagicNumber => [MAGIC_NUMBER]);

fn is_in_const_or_static(cx: &LateContext<'_>, expr: &Expr<'_>) -> bool {
    let owner = cx.tcx.hir_get_parent_item(expr.hir_id);
    match cx.tcx.hir_node_by_def_id(owner.def_id) {
        Node::Item(item) => matches!(
            item.kind,
            ItemKind::Const(..) | ItemKind::Static(..)
        ),
        Node::TraitItem(item) => {
            matches!(item.kind, TraitItemKind::Const(..))
        },
        Node::ImplItem(item) => {
            matches!(item.kind, ImplItemKind::Const(..))
        },
        _ => false,
    }
}

fn is_enum_discriminant(cx: &LateContext<'_>, expr: &Expr<'_>) -> bool {
    matches!(
        cx.tcx.parent_hir_node(expr.hir_id),
        Node::AnonConst(_)
    ) && matches!(
        cx.tcx.hir_node_by_def_id(
            cx.tcx.hir_get_parent_item(expr.hir_id).def_id
        ),
        Node::Item(item) if matches!(item.kind, ItemKind::Enum(..))
    )
}

impl LateLintPass<'_> for MagicNumber {
    fn check_expr(&mut self, cx: &LateContext<'_>, expr: &Expr<'_>) {
        if expr.span.in_external_macro(cx.sess().source_map()) {
            return;
        }

        if is_in_test(cx.tcx, expr.hir_id) {
            return;
        }

        let ExprKind::Lit(lit) = expr.kind else {
            return;
        };

        match lit.node {
            LitKind::Int(val, _) => {
                let v = val.get() as i128;
                if self.allowed_ints.iter().any(|&a| i128::from(a) == v) {
                    return;
                }
                // Check for negated literal: parent is Unary(Neg, _)
                if let Node::Expr(parent) = cx.tcx.parent_hir_node(expr.hir_id)
                    && let ExprKind::Unary(rustc_hir::UnOp::Neg, _) = parent.kind
                    && self.allowed_ints.iter().any(|&a| i128::from(a) == -v)
                {
                    return;
                }
            },
            LitKind::Float(sym, _) => {
                let s = sym.as_str();
                if self.allowed_floats.iter().any(|a| a == s) {
                    return;
                }
                if let Node::Expr(parent) = cx.tcx.parent_hir_node(expr.hir_id)
                    && let ExprKind::Unary(rustc_hir::UnOp::Neg, _) = parent.kind
                {
                    let neg = format!("-{s}");
                    if self.allowed_floats.iter().any(|a| a == &neg) {
                        return;
                    }
                }
            },
            _ => return,
        }

        if is_in_const_or_static(cx, expr) {
            return;
        }

        if is_enum_discriminant(cx, expr) {
            return;
        }

        // Array repeat count: [val; N] — the N is an AnonConst
        if matches!(
            cx.tcx.parent_hir_node(expr.hir_id),
            Node::AnonConst(_)
        ) {
            let grandparent = cx.tcx.hir_get_parent_item(expr.hir_id);
            if let Node::Expr(gp_expr) = cx.tcx.hir_node_by_def_id(grandparent.def_id) {
                if matches!(gp_expr.kind, ExprKind::Repeat(..)) {
                    return;
                }
            }
            // Also allow type-level constants (generic args, etc.)
            if matches!(
                cx.tcx.def_kind(grandparent.def_id),
                DefKind::AnonConst
            ) {
                return;
            }
        }

        span_lint_and_help(
            cx,
            MAGIC_NUMBER,
            expr.span,
            "magic number: numeric literals should be named constants \
             or configurable values",
            None,
            "extract this into a `const` (if the value is static) \
             or pass it as a struct field / function argument \
             (if it should be configurable)",
        );
    }
}
