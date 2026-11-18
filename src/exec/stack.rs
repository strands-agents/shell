use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

/// A recursive execution future with stack space for polling and cancellation.
struct Execution<F> {
    future: Option<Pin<Box<F>>>,
}

pub(super) fn run<F: Future>(future: F) -> impl Future<Output = F::Output> {
    Execution {
        future: Some(Box::pin(future)),
    }
}

impl<F: Future> Future for Execution<F> {
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        with_stack(|| self.future.as_mut().unwrap().as_mut().poll(cx))
    }
}

impl<F> Drop for Execution<F> {
    fn drop(&mut self) {
        with_stack(|| drop(self.future.take()));
    }
}

fn with_stack<R>(f: impl FnOnce() -> R) -> R {
    #[cfg(not(target_arch = "wasm32"))]
    {
        stacker::maybe_grow(1024 * 1024, 4 * 1024 * 1024, f)
    }
    #[cfg(target_arch = "wasm32")]
    {
        f()
    }
}
