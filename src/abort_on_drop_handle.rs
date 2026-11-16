use std::future::Future;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;

use tokio::task::JoinError;
use tokio::task::JoinHandle;

/// Join handle that aborts its task when dropped.
pub(crate) struct AbortOnDropHandle<T> {
    handle: JoinHandle<T>,
}

// Constructors live on `AbortOnDropHandle<()>` so the struct's type
// parameter does not have to be inferred at the call site.
impl AbortOnDropHandle<()> {
    pub(crate) fn spawn<F>(future: F) -> AbortOnDropHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        AbortOnDropHandle {
            handle: tokio::spawn(future),
        }
    }

    pub(crate) fn spawn_blocking<F, R>(f: F) -> AbortOnDropHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        AbortOnDropHandle {
            handle: tokio::task::spawn_blocking(f),
        }
    }
}

impl<T> Drop for AbortOnDropHandle<T> {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

impl<T> Future for AbortOnDropHandle<T> {
    type Output = Result<T, JoinError>;

    fn poll(
        mut self: Pin<&mut AbortOnDropHandle<T>>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<T, JoinError>> {
        Pin::new(&mut self.handle).poll(cx)
    }
}
