//@aux-build:tracing.rs
#![warn(clippy::uninstrumented_await)]
#![allow(
    unused,
    clippy::let_and_return,
    clippy::let_underscore_future,
    clippy::no_effect
)]

extern crate tracing;
use tracing::{Awaited, Instrument};

async fn io() -> u32 {
    0
}

// ---------------------------------------------------------------- should fire

async fn one_bare_among_spanned() {
    io().instrument("a").await;
    io().await;
    //~^ uninstrumented_await
}

async fn several_bare() {
    io().instrument("a").await;
    io().await;
    //~^ uninstrumented_await
    let _ = io().await + 1;
    //~^ uninstrumented_await
    io().in_current_span().await;
}

// A closure is judged on its own `.await`s: this one is itself mixed, so it
// fires, independently of the enclosing function.
fn mixed_closure() {
    let _ = async {
        io().instrument("a").await;
        io().await;
        //~^ uninstrumented_await
    };
}

// `awaited` covers the future it wraps; the *other* await here is still bare.
async fn awaited_does_not_excuse_its_neighbour() {
    io().awaited("a").await;
    io().await;
    //~^ uninstrumented_await
}

// ------------------------------------------------------------ should NOT fire

// No span anywhere: nobody claimed to care about await-level attribution here.
async fn nothing_instrumented() {
    io().await;
    io().await;
    let _ = io().await + io().await;
}

// Every `.await` is covered.
async fn fully_instrumented() {
    io().instrument("a").await;
    io().instrument("b").await;
    io().in_current_span().await;
}

// ix's `service_init::obs::Awaited`, applied directly.
async fn awaited_inline() {
    io().awaited("a").await;
    io().awaited("b").await;
}

// THE SPLIT FORM. The future is built in one statement and awaited in another,
// which is what people write when a future must exist before a branch. The
// awaited operand is a plain local, not a method call, so a name-matching lint
// reports this correct code as bare. The type test is what keeps it quiet: the
// local is an `Instrumented<_>` however it got that way.
//
// DO NOT SIMPLIFY THESE TWO FIXTURES. The first draft of `split_form` had no
// bare `.await` in it at all, which meant the function had nothing to trigger
// on and stayed silent whether the check worked or not — a test whose passing
// state is "nothing was reported" proves nothing until you have watched it
// report. It passed while the lint was still broken. The trailing inline
// `.await` below, and the `else` arm in `split_form_across_branch`, exist
// solely to give each function a reason to be judged. Remove either and the
// fixture goes back to passing vacuously.
async fn split_form() {
    // The trailing instrumented await supplies the trigger, so this function IS
    // judged. If the split awaits were mistaken for bare, they would fire.
    let first = io().awaited("first");
    let second = io().instrument("second");
    first.await;
    second.await;
    io().awaited("third").await;
}

// The discriminating one: `fut` is created before the branch and awaited inside
// it. This is the case that actually caught the false positive — under name
// matching, `fut.await` was reported while the `else` arm was not, so the
// function looked inconsistent and fired on correct code.
async fn split_form_across_branch(cond: bool) {
    let fut = io().awaited("work");
    if cond {
        fut.await;
    } else {
        io().awaited("other").await;
    }
}

// An inner async block with no spans of its own must not inherit the outer
// function's spans. The outer function still fires for its own bare `.await`.
async fn nested_block_does_not_inherit() {
    io().instrument("outer").await;
    let inner = async {
        io().await;
        io().await;
    };
    inner.await;
    //~^ uninstrumented_await
}

// The escape hatch, in the form ix's `allow_attributes_without_reason` forces.
async fn silenced_with_reason() {
    io().instrument("a").await;
    #[expect(clippy::uninstrumented_await, reason = "callee is #[instrument]ed")]
    io().await;
}

fn main() {}
