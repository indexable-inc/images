use clippy_utils::diagnostics::span_lint_and_help;
use clippy_utils::is_must_use_func_call;
use clippy_utils::ty::is_must_use_ty;
use rustc_hir::{Expr, ExprKind};
use rustc_lint::{LateContext, LateLintPass, LintContext};
use rustc_session::declare_lint_pass;
use rustc_span::sym;

declare_clippy_lint! {
    /// ### What it does
    /// Catches `drop(expr)` where `expr` is `#[must_use]` — either the type
    /// itself (e.g. `Result`) or the function call (e.g. a function annotated
    /// `#[must_use]`).
    ///
    /// ### Why restrict this?
    /// `drop()` explicitly consumes the value, bypassing both rustc's
    /// `unused_must_use` lint and clippy's `let_underscore_must_use` lint.
    /// This makes it a silent error-suppression pattern: the `Result` (or
    /// other must-use value) is discarded without any handling.
    ///
    /// ### Example
    /// ```rust,ignore
    /// drop(tx.send(result));
    /// drop(file.sync_all());
    /// ```
    /// Use instead:
    /// ```rust,ignore
    /// // Propagate the error
    /// tx.send(result)?;
    /// file.sync_all()?;
    /// ```
    #[clippy::version = "1.86.0"]
    pub DROP_MUST_USE,
    restriction,
    "`drop()` on a `#[must_use]` value silently discards it"
}

declare_lint_pass!(DropMustUse => [DROP_MUST_USE]);

impl<'tcx> LateLintPass<'tcx> for DropMustUse {
    fn check_expr(&mut self, cx: &LateContext<'tcx>, expr: &'tcx Expr<'_>) {
        if expr.span.in_external_macro(cx.sess().source_map()) {
            return;
        }

        // Match `drop(arg)` — a call to `std::mem::drop`
        let ExprKind::Call(path, [arg]) = expr.kind else {
            return;
        };
        let ExprKind::Path(ref qpath) = path.kind else {
            return;
        };
        let Some(def_id) = cx.qpath_res(qpath, path.hir_id).opt_def_id() else {
            return;
        };
        let Some(diag_name) = cx.tcx.get_diagnostic_name(def_id) else {
            return;
        };
        if diag_name != sym::mem_drop {
            return;
        }

        let arg_ty = cx.typeck_results().expr_ty(arg);

        if is_must_use_ty(cx, arg_ty) {
            span_lint_and_help(
                cx,
                DROP_MUST_USE,
                expr.span,
                format!(
                    "`drop()` on a value of `#[must_use]` type `{arg_ty}` silently \
                     discards it"
                ),
                None,
                "propagate the error with `?` or handle it explicitly",
            );
        } else if is_must_use_func_call(cx, arg) {
            span_lint_and_help(
                cx,
                DROP_MUST_USE,
                expr.span,
                "`drop()` on the result of a `#[must_use]` function silently \
                 discards it",
                None,
                "propagate the error with `?` or handle it explicitly",
            );
        }
    }
}
