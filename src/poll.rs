//! One poll of aria2 for the whole process.
//!
//! aria2 has no notification for byte-level progress, so the only way to show
//! a moving bar is to ask. Asked per open page, every stream would run its own
//! RPC calls, so the cost of going faster would be multiplied by however many
//! tabs were open, and a phone left on a bedside table would cost the same as
//! one being watched.
//!
//! Here it is one task. The RPC happens once per tick however many pages are
//! open, at the page's rate (`POLL_MS`) while anyone is watching and at the
//! notifier's (`IDLE_POLL_MS`) while nobody is. What a page is shown is the
//! web layer's business: each live stream draws the snapshots it reads, and
//! sends only what changed.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::{Notify, watch};

use crate::aria2::Download;
use crate::state::AppState;

pub struct Snapshot {
    /// `None` when aria2 did not answer. Readers that act on transitions skip
    /// these: an outage is not every download disappearing.
    pub downloads: Option<Arc<Vec<Download>>>,
}

#[derive(Clone)]
pub struct Poll {
    tx: Arc<watch::Sender<Arc<Snapshot>>>,
    viewers: Arc<AtomicUsize>,
    /// Wakes idle poller when a viewer connects.
    woken: Arc<Notify>,
    /// Set once the process is shutting down. Every open stream ends on it,
    /// because graceful shutdown waits for in-flight responses and a stream
    /// would otherwise hold the process open until it is killed.
    closing: Arc<watch::Sender<bool>>,
}

impl Poll {
    pub fn new() -> Self {
        let (tx, _) = watch::channel(Arc::new(Snapshot { downloads: None }));
        Self {
            tx: Arc::new(tx),
            viewers: Arc::new(AtomicUsize::new(0)),
            woken: Arc::new(Notify::new()),
            closing: Arc::new(watch::channel(false).0),
        }
    }

    /// Ends every stream, including ones opened after this call.
    pub fn close(&self) {
        self.closing.send_replace(true);
    }

    /// Every snapshot from now on, for a reader that is not a page: the
    /// notifier and the reconciler.
    pub fn subscribe(&self) -> watch::Receiver<Arc<Snapshot>> {
        self.tx.subscribe()
    }

    /// Whether anyone has a page open, which is the only input to how hard
    /// the poller works.
    pub fn watched(&self) -> bool {
        self.viewers.load(Ordering::Relaxed) > 0
    }

    /// An open page: the poller samples at the page's rate until it drops.
    pub fn viewer(&self) -> Viewer {
        self.viewers.fetch_add(1, Ordering::Relaxed);
        self.woken.notify_one();
        Viewer {
            rx: self.tx.subscribe(),
            closing: self.closing.subscribe(),
            viewers: Arc::clone(&self.viewers),
        }
    }
}

/// One open page. Dropping it decrements the viewer count.
pub struct Viewer {
    rx: watch::Receiver<Arc<Snapshot>>,
    closing: watch::Receiver<bool>,
    viewers: Arc<AtomicUsize>,
}

impl Viewer {
    /// The next snapshot, or `None` once the process is shutting down.
    pub async fn next(&mut self) -> Option<Arc<Snapshot>> {
        tokio::select! {
            changed = self.rx.changed() => {
                changed.ok()?;
                Some(self.rx.borrow_and_update().clone())
            }
            _ = self.closing.wait_for(|closing| *closing) => None,
        }
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        self.viewers.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Sample aria2 forever, at the page's rate while anyone is watching and at
/// the notifier's while nobody is.
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        let mut failures = 0u32;
        loop {
            let watched = state.poll.watched();
            if watched {
                tokio::time::sleep(state.config.poll_interval).await;
            } else {
                tokio::select! {
                    _ = tokio::time::sleep(state.config.idle_poll_interval) => {}
                    _ = state.poll.woken.notified() => {}
                }
            }

            let snapshot = match state.aria2.downloads().await {
                Ok(downloads) => {
                    if failures > 0 {
                        tracing::info!(failures, "poll: aria2 answering again");
                    }
                    failures = 0;
                    Snapshot {
                        downloads: Some(Arc::new(downloads)),
                    }
                }
                Err(e) => {
                    failures += 1;
                    if failures == 1 {
                        tracing::warn!(error = %e, "poll: aria2 poll failed, retrying");
                    } else {
                        tracing::debug!(error = %e, failures, "poll: aria2 poll failed");
                    }
                    // One slow answer is not an outage worth flipping every
                    // open page over for.
                    if failures < OUTAGE_AFTER {
                        continue;
                    }
                    Snapshot { downloads: None }
                }
            };
            let _ = state.poll.tx.send(Arc::new(snapshot));
        }
    });
}

/// Consecutive failed polls before aria2 is reported unreachable.
const OUTAGE_AFTER: u32 = 3;

#[cfg(test)]
impl Poll {
    pub(crate) fn publish(&self, snapshot: Snapshot) {
        let _ = self.tx.send(Arc::new(snapshot));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole cost model: an open page is what makes this sample fast, and
    /// the last one closing is what lets it stop. A guard that leaked would
    /// leave a process polling ten times a second at nobody, forever.
    #[test]
    fn a_page_opening_and_closing_is_what_moves_the_rate() {
        let poll = Poll::new();
        assert!(!poll.watched());

        let one = poll.viewer();
        let two = poll.viewer();
        assert!(poll.watched());

        drop(one);
        assert!(poll.watched(), "one page closing is not every page closing");
        drop(two);
        assert!(!poll.watched());
    }

    /// Graceful shutdown waits for every response to finish, and a stream
    /// never does on its own.
    #[tokio::test]
    async fn closing_ends_open_streams_and_ones_opened_afterwards() {
        let poll = Poll::new();
        let mut open = poll.viewer();
        poll.close();
        assert!(open.next().await.is_none());
        assert!(poll.viewer().next().await.is_none());
    }
}
