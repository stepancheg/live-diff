use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::sync::mpsc::Receiver;
use tokio_util::task::AbortOnDropHandle;

use crate::repo::repo::DiffTool;
use crate::repo::sl::smartlog::SmartlogView;
use crate::repo::sl::smartlog::fetch_smartlog;
use crate::util::notify_pair::NotifyRx;
use crate::util::notify_pair::NotifyTx;

/// One smartlog snapshot from the background poll, tagged with the
/// epoch observed when the fetch started.
pub(crate) struct SmartlogPollMsg {
    pub(crate) epoch: u64,
    pub(crate) view: SmartlogView,
}

/// Smartlog poll task and the ends the UI loop uses.
///
/// Spawn this before the event loop starts. The task handle must
/// stay alive for the whole run.
pub(crate) struct SmartlogActor {
    pub(crate) live: Arc<AtomicBool>,
    pub(crate) epoch: Arc<AtomicU64>,
    pub(crate) wake_tx: NotifyTx,
    pub(crate) views_rx: Option<Receiver<SmartlogPollMsg>>,
    _task: Option<AbortOnDropHandle<()>>,
}

impl SmartlogActor {
    pub(crate) fn spawn(tool: Option<DiffTool>, interval: Duration) -> SmartlogActor {
        let live = Arc::new(AtomicBool::new(false));
        let epoch = Arc::new(AtomicU64::new(0));
        let (wake_tx, wake_rx) = NotifyTx::pair();
        let (views_rx, task) = match tool {
            Some(DiffTool::Sl) => {
                let (tx, rx) = mpsc::channel(1);
                let task = AbortOnDropHandle::new(tokio::spawn(smartlog_poll_loop(
                    Arc::clone(&live),
                    Arc::clone(&epoch),
                    wake_rx,
                    interval,
                    tx,
                )));
                (Some(rx), Some(task))
            }
            _ => (None, None),
        };
        SmartlogActor {
            live,
            epoch,
            wake_tx,
            views_rx,
            _task: task,
        }
    }
}

/// Re-query `sl smartlog` on `interval` while `live` is set.
async fn smartlog_poll_loop(
    live: Arc<AtomicBool>,
    epoch: Arc<AtomicU64>,
    wake: NotifyRx,
    interval: Duration,
    tx: mpsc::Sender<SmartlogPollMsg>,
) {
    loop {
        if !live.load(Ordering::SeqCst) {
            wake.notified().await;
            continue;
        }
        tokio::select! {
            () = tokio::time::sleep(interval) => {}
            () = wake.notified() => {
                continue;
            }
        }
        if !live.load(Ordering::SeqCst) {
            continue;
        }
        let seen = epoch.load(Ordering::SeqCst);
        let view = match tokio::task::spawn_blocking(fetch_smartlog).await {
            Ok(Ok(view)) => view,
            Ok(Err(_)) => continue,
            Err(_) => continue,
        };
        if !live.load(Ordering::SeqCst) {
            continue;
        }
        if epoch.load(Ordering::SeqCst) != seen {
            continue;
        }
        let msg = SmartlogPollMsg { epoch: seen, view };
        if tx.send(msg).await.is_err() {
            return;
        }
    }
}
