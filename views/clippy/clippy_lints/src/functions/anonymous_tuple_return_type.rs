use crate::functions::ANONYMOUS_TUPLE_RETURN_TYPE;
use clippy_utils::diagnostics::span_lint_and_help;
use clippy_utils::{is_from_proc_macro, is_trait_impl_item};
use rustc_hir::intravisit::{self, Visitor, VisitorExt};
use rustc_hir::{
    self as hir, AmbigArg, AssocItemConstraintKind, FnDecl, FnRetTy, GenericArgs, GenericArgsParentheses, HirId, Term,
    Ty, TyKind,
};
use rustc_lint::{LateContext, LintContext};
use rustc_span::Span;
use std::ops::ControlFlow;

struct AnonymousTupleVisitor<'cx, 'tcx> {
    cx: &'cx LateContext<'tcx>,
}

impl<'tcx> Visitor<'tcx> for AnonymousTupleVisitor<'_, 'tcx> {
    type NestedFilter = intravisit::nested_filter::None;
    type Result = ControlFlow<Span>;

    fn visit_ty(&mut self, ty: &'tcx Ty<'tcx, AmbigArg>) -> Self::Result {
        let unambig_ty = ty.as_unambig_ty();

        if unambig_ty.span.in_external_macro(self.cx.sess().source_map()) {
            return ControlFlow::Continue(());
        }

        if let TyKind::Tup(fields) = unambig_ty.kind
            && !fields.is_empty()
        {
            return ControlFlow::Break(unambig_ty.span);
        }

        intravisit::walk_ty(self, ty)
    }

    /// Parenthesised `Fn`-trait sugar lowers the callable's argument list into a
    /// tuple: `FnOnce(io::Error) -> E` is `FnOnce<(io::Error,), Output = E>`. That
    /// tuple is the callable's parameter list, not a value this function returns,
    /// so only the `Output` term is a return position worth checking.
    fn visit_generic_args(&mut self, generic_args: &'tcx GenericArgs<'tcx>) -> Self::Result {
        if generic_args.parenthesized == GenericArgsParentheses::ParenSugar {
            for constraint in generic_args.constraints {
                if let AssocItemConstraintKind::Equality { term: Term::Ty(output) } = constraint.kind {
                    self.visit_ty_unambig(output)?;
                }
            }
            return ControlFlow::Continue(());
        }

        intravisit::walk_generic_args(self, generic_args)
    }

    /// A function pointer nested in the return type (`fn(A) -> R`) returns `R`
    /// alone; its parameters are arguments, so a tuple among them is not returned.
    fn visit_fn_decl(&mut self, decl: &'tcx FnDecl<'tcx>) -> Self::Result {
        if let FnRetTy::Return(ret_ty) = decl.output {
            return self.visit_ty_unambig(ret_ty);
        }

        ControlFlow::Continue(())
    }
}

fn anonymous_tuple_span<'tcx>(cx: &LateContext<'tcx>, ret_ty: &'tcx Ty<'tcx>) -> Option<Span> {
    let mut visitor = AnonymousTupleVisitor { cx };
    visitor.visit_ty_unambig(ret_ty).break_value()
}

fn check_ret_ty<'tcx>(cx: &LateContext<'tcx>, ret_ty: &'tcx Ty<'tcx>) {
    if let Some(tuple_span) = anonymous_tuple_span(cx, ret_ty) {
        span_lint_and_help(
            cx,
            ANONYMOUS_TUPLE_RETURN_TYPE,
            tuple_span,
            "return type contains an anonymous tuple",
            None,
            "use a named struct to make the returned fields self-documenting",
        );
    }
}

pub(super) fn check_fn<'tcx>(cx: &LateContext<'tcx>, decl: &'tcx FnDecl<'tcx>, hir_id: HirId, span: Span) {
    if span.in_external_macro(cx.sess().source_map()) || is_trait_impl_item(cx, hir_id) {
        return;
    }

    if let FnRetTy::Return(ret_ty) = decl.output {
        check_ret_ty(cx, ret_ty);
    }
}

pub(super) fn check_trait_item<'tcx>(cx: &LateContext<'tcx>, item: &'tcx hir::TraitItem<'tcx>) {
    if !item.span.in_external_macro(cx.sess().source_map())
        && !is_from_proc_macro(cx, item)
        && let hir::TraitItemKind::Fn(ref sig, hir::TraitFn::Required(_)) = item.kind
        && let FnRetTy::Return(ret_ty) = sig.decl.output
    {
        check_ret_ty(cx, ret_ty);
    }
}
