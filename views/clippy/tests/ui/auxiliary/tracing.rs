//! Minimal stand-in for the parts of `tracing` this lint recognises. Named
//! `tracing` on purpose: the lint matches `tracing::instrument::Instrumented`
//! by def path, so the fixture has to carry that path to exercise the type test.
#![allow(unused)]

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

pub mod instrument {
    use super::{Context, Future, Pin, Poll};

    pub struct Instrumented<F> {
        inner: Pin<Box<F>>,
    }

    impl<F> Instrumented<F> {
        pub fn new(inner: F) -> Self {
            Self { inner: Box::pin(inner) }
        }
    }

    impl<F: Future> Future for Instrumented<F> {
        type Output = F::Output;
        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            self.get_mut().inner.as_mut().poll(cx)
        }
    }
}

pub use instrument::Instrumented;

pub trait Instrument: Sized {
    fn instrument(self, _span: &str) -> Instrumented<Self> {
        Instrumented::new(self)
    }
    fn in_current_span(self) -> Instrumented<Self> {
        Instrumented::new(self)
    }
}
impl<F: Future> Instrument for F {}

/// Mirrors ix's `service_init::obs::Awaited`.
pub trait Awaited: Future + Sized {
    fn awaited(self, _name: &'static str) -> Instrumented<Self> {
        Instrumented::new(self)
    }
}
impl<F: Future> Awaited for F {}
