use std::sync::Arc;

use tokio::sync::Notify;

/// Sending half of a one-shot-coalescing wake signal.
pub(crate) struct NotifyTx {
    notify: Arc<Notify>,
}

/// Receiving half of a one-shot-coalescing wake signal.
pub(crate) struct NotifyRx {
    notify: Arc<Notify>,
}

impl NotifyTx {
    /// A notify with no waiter is stored, so the next `notified` returns at once.
    /// Further notifies while one is already stored are coalesced.
    pub(crate) fn pair() -> (NotifyTx, NotifyRx) {
        let notify = Arc::new(Notify::new());
        (
            NotifyTx {
                notify: Arc::clone(&notify),
            },
            NotifyRx { notify },
        )
    }

    pub(crate) fn notify_one(&self) {
        self.notify.notify_one();
    }
}

impl NotifyRx {
    pub(crate) async fn notified(&self) {
        self.notify.notified().await;
    }
}
