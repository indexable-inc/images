use clippy_utils::diagnostics::span_lint_and_help;
use clippy_utils::res::MaybeDef;
use clippy_utils::sym;
use rustc_hir::{Expr, ExprKind};
use rustc_lint::{LateContext, LateLintPass, LintContext};
use rustc_middle::ty::{self, Ty};
use rustc_session::declare_lint_pass;

declare_clippy_lint! {
    /// ### What it does
    /// Catches `.unwrap_or(0)`, `.unwrap_or(T::MAX)`, `.unwrap_or(T::MIN)`,
    /// `.unwrap_or_default()`, and `.unwrap_or_else(..)` on
    /// `Result<_, TryFromIntError>`, i.e. the result of fallible integer
    /// conversions via `TryFrom`/`TryInto`.
    ///
    /// ### Why restrict this?
    /// Substituting a default for an out-of-range integer conversion hides the
    /// overflow: `intTy::try_from(x).unwrap_or(default)` silently clamps or
    /// zeroes a value that did not fit, which causes odd, hard-to-trace
    /// behavior far from the conversion. An out-of-range conversion should be
    /// an error that is propagated or reported, not silently defaulted.
    ///
    /// ### Example
    /// ```rust,ignore
    /// let n: u8 = u8::try_from(big_value).unwrap_or(0);
    /// let n: i16 = i16::try_from(x).unwrap_or(i16::MAX);
    /// let n: u32 = u32::try_from(x).unwrap_or_default();
    /// let n: usize = usize::try_from(x).unwrap_or_else(|_| 0);
    /// ```
    /// Use instead:
    /// ```rust,ignore
    /// let n: u8 = u8::try_from(big_value)?;
    /// ```
    /// If clamping is genuinely intended, write it explicitly (for example
    /// `x.min(u8::MAX as _) as u8`) with a comment, or
    /// `#[allow(clippy::fallible_int_fallback)]` with a reason.
    #[clippy::version = "1.86.0"]
    pub FALLIBLE_INT_FALLBACK,
    restriction,
    "`.unwrap_or` / `.unwrap_or_default` / `.unwrap_or_else` on fallible integer conversion silently loses data"
}

declare_lint_pass!(FallibleIntFallback => [FALLIBLE_INT_FALLBACK]);

/// Returns `true` if `ty` is `std::num::TryFromIntError`.
fn is_try_from_int_error(cx: &LateContext<'_>, ty: Ty<'_>) -> bool {
    if let ty::Adt(adt_def, _) = ty.kind() {
        let path = cx.tcx.def_path_str(adt_def.did());
        path == "core::num::error::TryFromIntError"
            || path == "std::num::TryFromIntError"
    } else {
        false
    }
}

/// If `recv_ty` is `Result<T, E>`, returns `E`.
fn result_error_ty<'tcx>(
    cx: &LateContext<'tcx>,
    recv_ty: Ty<'tcx>,
) -> Option<Ty<'tcx>> {
    if recv_ty.is_diag_item(cx, sym::Result) {
        if let ty::Adt(_, args) = recv_ty.kind() {
            return Some(args.type_at(1));
        }
    }
    None
}

impl LateLintPass<'_> for FallibleIntFallback {
    fn check_expr(&mut self, cx: &LateContext<'_>, expr: &Expr<'_>) {
        if expr.span.in_external_macro(cx.sess().source_map()) {
            return;
        }

        let ExprKind::MethodCall(method, recv, _, _) = expr.kind else {
            return;
        };

        // `unwrap_or`, `unwrap_or_default`, and `unwrap_or_else` all silently
        // substitute a value when the conversion failed. The receiver-typing
        // check below is what distinguishes a fallible integer conversion from
        // an unrelated `Result::unwrap_or`, so this stays type-aware.
        let method_name = match method.ident.name {
            sym::unwrap_or => "unwrap_or(...)",
            sym::unwrap_or_default => "unwrap_or_default()",
            sym::unwrap_or_else => "unwrap_or_else(...)",
            _ => return,
        };

        let recv_ty = cx.typeck_results().expr_ty(recv);
        let Some(err_ty) = result_error_ty(cx, recv_ty) else {
            return;
        };

        if !is_try_from_int_error(cx, err_ty) {
            return;
        }

        span_lint_and_help(
            cx,
            FALLIBLE_INT_FALLBACK,
            expr.span,
            format!(
                "`.{method_name}` silently substitutes a default for an \
                 out-of-range integer conversion, hiding the overflow"
            ),
            None,
            "handle the conversion error instead (propagate with `?`, or report it); \
             if clamping is genuinely intended, write it explicitly \
             (e.g. `x.min(MAX)` before the cast) with a comment, or \
             `#[allow(clippy::fallible_int_fallback)]` with a reason",
        );
    }
}
