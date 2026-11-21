use std::collections::HashMap;
use std::time::Instant;

use tokio::sync::mpsc;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::task::AbortOnDropHandle;

use crate::repo::forge::Forge;
use crate::repo::pr_status::PR_STATUS_REFRESH;
use crate::repo::pr_status::PrStatus;
use crate::repo::pr_status::fetch_pr_statuses;

/// Ask the status worker to load review/CI state for these PRs.
pub(crate) struct PrStatusJob {
    pub(crate) forge: Forge,
    pub(crate) numbers: Vec<u64>,
    /// When true, fetch even if this forge and PR set were loaded
    /// inside [`PR_STATUS_REFRESH`].
    pub(crate) bypass_cooldown: bool,
}

/// One completed fetch. Empty maps are not sent.
pub(crate) struct PrStatusBatch {
    pub(crate) forge: Forge,
    pub(crate) statuses: HashMap<u64, PrStatus>,
}

/// PR-status task and the ends the UI loop uses.
///
/// Spawn this before the event loop starts. The task handle must
/// stay alive for the whole run.
pub(crate) struct PrStatusActor {
    pub(crate) job_tx: UnboundedSender<PrStatusJob>,
    pub(crate) result_rx: Receiver<PrStatusBatch>,
    _task: AbortOnDropHandle<()>,
}

impl PrStatusActor {
    pub(crate) fn spawn() -> PrStatusActor {
        let (job_tx, job_rx) = mpsc::unbounded_channel();
        let (result_tx, result_rx) = mpsc::channel(8);
        let task = AbortOnDropHandle::new(tokio::spawn(pr_status_loop(job_rx, result_tx)));
        PrStatusActor {
            job_tx,
            result_rx,
            _task: task,
        }
    }
}

/// Fetch PR statuses off the UI task. Jobs are coalesced to the
/// latest set. A repeat of the same set inside [`PR_STATUS_REFRESH`]
/// is skipped so the 0.5s smartlog poll does not hammer the forge.
async fn pr_status_loop(
    mut jobs: UnboundedReceiver<PrStatusJob>,
    results: mpsc::Sender<PrStatusBatch>,
) {
    let mut last: Option<(Forge, Vec<u64>, Instant)> = None;
    while let Some(mut job) = jobs.recv().await {
        while let Ok(next) = jobs.try_recv() {
            let bypass_cooldown = job.bypass_cooldown || next.bypass_cooldown;
            job = next;
            job.bypass_cooldown = bypass_cooldown;
        }
        let fresh = match &last {
            Some((forge, numbers, at)) => {
                *forge == job.forge && *numbers == job.numbers && at.elapsed() < PR_STATUS_REFRESH
            }
            None => false,
        };
        if fresh {
            if !job.bypass_cooldown {
                continue;
            }
        }
        let statuses = fetch_pr_statuses(&job.forge, &job.numbers).await;
        last = Some((job.forge.clone(), job.numbers, Instant::now()));
        if statuses.is_empty() {
            continue;
        }
        let batch = PrStatusBatch {
            forge: job.forge,
            statuses,
        };
        if results.send(batch).await.is_err() {
            return;
        }
    }
}
