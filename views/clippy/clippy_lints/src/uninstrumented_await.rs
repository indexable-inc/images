use clippy_config::Conf;
use clippy_utils::desugar_await;
use clippy_utils::diagnostics::span_lint_hir_and_then;
use clippy_utils::visitors::for_each_expr_without_closures;
use rustc_hir::{Body, Expr, ExprKind, HirId};
use rustc_lint::{LateContext, LateLintPass, LintContext};
use rustc_middle::ty;
use rustc_session::impl_lint_pass;
use rustc_span::Span;
use std::ops::ControlFlow;

/// The type every `tracing` span combinator returns. Recognising the *type*
/// rather than the call is what lets this lint see a future that was given a
/// span somewhere other than immediately before the `.await`.
const INSTRUMENTED: &str = "tracing::instrument::Instrumented";

declare_clippy_lint! {
    /// ### What it does
    /// Within one function, closure, or `async` block, checks that if *any*
    /// `.await` is covered by a span then *every* `.await` is. Reports the ones
    /// that are not.
    ///
    /// ### Why restrict this?
    /// A partly instrumented function produces the worst kind of trace: one
    /// that looks complete and is not. The spans that are there account for
    /// some of the wall clock, the bare `.await`s account for none of it, and
    /// nothing in the trace says the difference exists. Time spent at a bare
    /// `.await` does not vanish from the total — it shows up as unexplained
    /// gap, so the trace actively points away from the real cost.
    ///
    /// Whoever wrote the spans that *are* there already decided that
    /// await-level attribution matters in this function. This lint holds the
    /// rest of the function to that decision. A function with no spans at all
    /// is deliberately **not** linted: nobody claimed to care, and
    /// blanket-instrumenting every `.await` in a program is not the goal.
    ///
    /// ### What counts as covered
    /// Primarily a *type* test: the awaited future is a
    /// `tracing::instrument::Instrumented<_>`. That is what `.instrument(..)`,
    /// `.in_current_span()` and a project's own combinator all return, so it
    /// holds however the future reached the `.await` — including when it was
    /// built into a local first:
    ///
    /// ```rust,ignore
    /// let fut = work().awaited("step");  // Instrumented<_>
    /// if cond { fut.await } else { .. }  // still covered
    /// ```
    ///
    /// A configurable method-name list (`instrumenting-methods`) is also
    /// honoured, for a combinator that wraps a future without returning
    /// `Instrumented`.
    ///
    /// ### Known problem
    /// The lint cannot see that the awaited callee is itself annotated with
    /// `#[tracing::instrument]`. That attribute is a proc macro and is gone by
    /// the time the lint runs, and for a callee in another crate the body is
    /// not available at all. Awaiting such a function is a false positive; the
    /// intended answer is an `#[expect]` naming that reason, which is visible
    /// in review, greppable, and self-deleting once it stops being true.
    ///
    /// `.await`s produced by a macro expansion are ignored entirely — they are
    /// neither reported nor counted as instrumentation.
    ///
    /// ### Example
    /// ```rust,ignore
    /// async fn ingest(&self) -> Result<()> {
    ///     self.stage().instrument(trace_span!("ingest.stage")).await?;
    ///     // Not in the trace. Nothing says so.
    ///     self.receive_body().await?;
    ///     Ok(())
    /// }
    /// ```
    /// Use instead:
    /// ```rust,ignore
    /// async fn ingest(&self) -> Result<()> {
    ///     self.stage().instrument(trace_span!("ingest.stage")).await?;
    ///     self.receive_body().awaited("ingest.receive_body").await?;
    ///     Ok(())
    /// }
    /// ```
    #[clippy::version = "1.86.0"]
    pub UNINSTRUMENTED_AWAIT,
    restriction,
    "an `.await` with no span in a function that spans its other `.await`s"
}

pub struct UninstrumentedAwait {
    instrumenting_methods: Vec<String>,
}

impl UninstrumentedAwait {
    pub fn new(conf: &'static Conf) -> Self {
        Self {
            instrumenting_methods: conf.instrumenting_methods.clone(),
        }
    }

    /// Whether `operand` — the future an `.await` is applied to — carries a span.
    fn is_instrumented(&self, cx: &LateContext<'_>, operand: &Expr<'_>) -> bool {
        // The type test. Independent of how the future got here, so a span
        // attached in a previous statement, behind a branch, or by a helper
        // this lint has never heard of all count.
        if let ty::Adt(adt_def, _) = cx.typeck_results().expr_ty(operand).kind() {
            let did = adt_def.did();
            // Match the bare type name as well as the full path. Erring toward
            // "this is instrumented" costs a missed report; erring the other way
            // reports correct code, which is the failure this lint cannot afford.
            if cx.tcx.def_path_str(did) == INSTRUMENTED || cx.tcx.item_name(did).as_str() == "Instrumented" {
                return true;
            }
        }

        // Name fallback, for a combinator that wraps a future in something
        // other than `Instrumented`. Configurable so a project can name its own
        // without waiting on a clippy release.
        if let ExprKind::MethodCall(method, ..) = operand.kind {
            let name = method.ident.name.as_str();
            return self.instrumenting_methods.iter().any(|allowed| allowed == name);
        }

        false
    }
}

impl_lint_pass!(UninstrumentedAwait => [UNINSTRUMENTED_AWAIT]);

impl<'tcx> LateLintPass<'tcx> for UninstrumentedAwait {
    // One walk per body. `check_body` fires separately for every closure and
    // `async` block, and `for_each_expr_without_closures` does not descend into
    // them, so each is judged on its own `.await`s. An inner closure does not
    // inherit its enclosing function's spans, and does not pollute them either.
    fn check_body(&mut self, cx: &LateContext<'tcx>, body: &Body<'tcx>) {
        let mut instrumented: Option<Span> = None;
        let mut bare: Vec<(HirId, Span)> = Vec::new();

        for_each_expr_without_closures(body.value, |expr| {
            // `desugar_await` yields the awaited operand, and returns `None`
            // when that operand came from a macro expansion. That is the
            // behaviour we want for a helper macro: an `.await` the lint cannot
            // see inside is neither an offence nor a credit.
            if let Some(operand) = desugar_await(expr)
                && !expr.span.in_external_macro(cx.sess().source_map())
            {
                if self.is_instrumented(cx, operand) {
                    instrumented.get_or_insert(expr.span);
                } else {
                    bare.push((expr.hir_id, expr.span));
                }
            }
            ControlFlow::<()>::Continue(())
        });

        // No span anywhere in this body: nobody claimed to care. Stay quiet.
        let Some(instrumented_span) = instrumented else {
            return;
        };

        for (hir_id, span) in bare {
            span_lint_hir_and_then(
                cx,
                UNINSTRUMENTED_AWAIT,
                hir_id,
                span,
                "this `.await` is not covered by a span, so its time is invisible in the trace",
                |diag| {
                    diag.span_note(
                        instrumented_span,
                        "this function instruments other `.await`s, so its trace reads as complete",
                    );
                    diag.help(
                        "give the future a span before awaiting it (`.awaited(\"...\")`, or \
                         `.instrument(tracing::trace_span!(\"...\"))`), or, if the callee is already \
                         `#[instrument]`ed, `#[expect(clippy::uninstrumented_await, reason = \"...\")]` \
                         on the statement",
                    );
                },
            );
        }
    }
}
