//! Shared state, cloned into every handler. Every field is cheap to clone —
//! an `Arc`, a `reqwest::Client`, or a map behind one.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::activity::Activity;
use crate::aria2::Aria2;
use crate::auth::session::Sessions;
use crate::config::Config;
use crate::poll::Poll;

/// Downloads whose removal has been asked for and that aria2 has not yet let
/// go of.
///
/// aria2 does not forget a download the moment it is told to, and the list is
/// redrawn from aria2 once a second — so a press had no effect anything could
/// see until the row eventually vanished, which is what made the button read
/// as broken. A gid in here renders as "clearing" with its controls replaced
/// by a spinner, from every render of every open page rather than only the one
/// that was clicked: the state belongs to the download, not to the browser
/// that asked.
#[derive(Clone, Default)]
pub struct Clearing(Arc<Mutex<HashSet<String>>>);

impl Clearing {
    /// A poisoned lock is recovered from rather than propagated, for the same
    /// reason `Activity` does it: losing this set would leave every row that
    /// was mid-removal stuck showing controls that do nothing.
    fn lock(&self) -> std::sync::MutexGuard<'_, HashSet<String>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `true` when this call is the one that marked it. A second press while
    /// the first removal is still in flight is not a second removal.
    pub fn begin(&self, gid: &str) -> bool {
        self.lock().insert(gid.to_string())
    }

    pub fn end(&self, gid: &str) {
        self.lock().remove(gid);
    }

    pub fn contains(&self, gid: &str) -> bool {
        self.lock().contains(gid)
    }

    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }
}

/// Downloads this process is part-way through adding.
///
/// An unfinished add is identified by what aria2 holds — paused, with no
/// `select-file` — and that description fits a *live* add just as exactly as
/// an abandoned one, for the whole second or two it takes. Without this the
/// reconciler in `resume` would adopt the add already running beside it, and
/// the two would race to pause, narrow and start the same download.
///
/// Held rather than inferred because there is nothing to infer from: aria2
/// cannot say which of two processes asked, and the answer is only ever
/// interesting to the one that did.
#[derive(Clone, Default)]
pub struct Adding(Arc<Mutex<HashSet<String>>>);

impl Adding {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashSet<String>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Claims `gid` until the returned guard drops, which is what makes the
    /// error paths safe: a finish that fails half way still has to let the
    /// reconciler have another go, and there is no `?` that can skip a drop.
    pub fn claim(&self, gid: &str) -> Claim {
        self.lock().insert(gid.to_string());
        Claim {
            set: self.clone(),
            gid: gid.to_string(),
        }
    }

    pub fn contains(&self, gid: &str) -> bool {
        self.lock().contains(gid)
    }
}

/// One claimed gid, released on drop. See [`Adding::claim`].
pub struct Claim {
    set: Adding,
    gid: String,
}

impl Drop for Claim {
    fn drop(&mut self) {
        self.set.lock().remove(&self.gid);
    }
}

/// Downloads whose record is kept at once, oldest dropped first.
const MAX_PREEXISTING: usize = 200;

/// For each download, which of its listed files were already on disk before
/// aria2 opened any of them.
///
/// aria2 runs with `--check-integrity`, so a file already in the library at a
/// path a torrent lists is verified and counted as complete, exactly like one
/// it downloaded. Re-adding a season pack into the folder that holds some of
/// its episodes is the ordinary case, and nothing aria2 reports afterwards
/// tells those episodes apart from the ones it fetched. The difference is only
/// visible before the download starts, which is when `add::finish` looks.
/// Cancelling and the garbage sweep leave these files alone.
///
/// Kept across restarts in `STATE_DIR` when it is set: the sweep after a
/// restart is exactly when it is needed, since the notifier sweeps every
/// finished download it finds as it starts. A download with no record (one
/// added before the record was kept, or with no `STATE_DIR`) is `known` as
/// `None`, and deleting for it is cautious (`library`).
#[derive(Clone, Default)]
pub struct Preexisting {
    inner: Arc<Mutex<PreexistingInner>>,
    /// Where the record is kept; none keeps it in memory.
    file: Option<Arc<PathBuf>>,
    /// One write at a time, so an older record never lands after a newer one.
    writing: Arc<tokio::sync::Mutex<()>>,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct PreexistingInner {
    files: HashMap<String, HashSet<PathBuf>>,
    order: VecDeque<String>,
}

impl Preexisting {
    /// The record kept in `dir`, or an empty one kept there from now on. A
    /// record that cannot be read is started afresh, with a warning: every
    /// download in it then has no record, which deletes less, not more.
    pub async fn load(dir: Option<&std::path::Path>) -> Self {
        let Some(dir) = dir else {
            return Self::default();
        };
        let file = dir.join("preexisting.json");
        let read = match tokio::fs::read(&file).await {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| e.to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(PreexistingInner::default()),
            Err(e) => Err(e.to_string()),
        };
        let inner = read.unwrap_or_else(|error| {
            tracing::warn!(path = %file.display(), %error, "the record of files already in the library could not be read; starting afresh");
            PreexistingInner::default()
        });
        Self {
            inner: Arc::new(Mutex::new(inner)),
            file: Some(Arc::new(file)),
            writing: Arc::default(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, PreexistingInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub async fn record(&self, gid: &str, files: HashSet<PathBuf>) {
        {
            let mut inner = self.lock();
            if inner.files.insert(gid.to_string(), files).is_none() {
                inner.order.push_back(gid.to_string());
                if inner.order.len() > MAX_PREEXISTING
                    && let Some(oldest) = inner.order.pop_front()
                {
                    inner.files.remove(&oldest);
                }
            }
        }
        self.save().await;
    }

    /// `None` when nothing was recorded for this download.
    pub fn known(&self, gid: &str) -> Option<HashSet<PathBuf>> {
        self.lock().files.get(gid).cloned()
    }

    pub async fn forget(&self, gid: &str) {
        {
            let mut inner = self.lock();
            if inner.files.remove(gid).is_none() {
                return;
            }
            inner.order.retain(|g| g != gid);
        }
        self.save().await;
    }

    /// Writes the record whole, beside its file and then over it, so a
    /// process stopped part-way leaves the last record rather than half of
    /// one. A failure is logged: the record in memory still holds.
    async fn save(&self) {
        let Some(file) = &self.file else {
            return;
        };
        let _writing = self.writing.lock().await;
        let json = serde_json::to_vec(&*self.lock());
        let json = match json {
            Ok(json) => json,
            Err(e) => {
                tracing::warn!(error = %e, "could not encode the record of files already in the library");
                return;
            }
        };
        let partial = file.with_extension("json.partial");
        let written = match tokio::fs::write(&partial, json).await {
            Ok(()) => tokio::fs::rename(&partial, file.as_path()).await,
            Err(e) => Err(e),
        };
        if let Err(e) = written {
            tracing::warn!(path = %file.display(), error = %e, "could not keep the record of files already in the library");
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub aria2: Aria2,
    pub sessions: Sessions,
    /// The single poll of aria2 every page and the notifier read from, rather
    /// than each running its own.
    pub poll: Poll,
    /// What happened to each download, for the part of it aria2 does not know
    /// — see `activity`.
    pub activity: Activity,
    /// Removals in flight — see [`Clearing`].
    pub clearing: Clearing,
    /// Adds in flight — see [`Adding`].
    pub adding: Adding,
    /// Files already on disk before each download started — see
    /// [`Preexisting`].
    pub preexisting: Preexisting,
    /// Hydra and Telegram. One client, so the connection pool and the TLS
    /// session cache are shared.
    pub http: reqwest::Client,
}

#[cfg(test)]
impl AppState {
    /// The service against an aria2 at `aria2_rpc_url`, with the test
    /// configuration and nothing in flight.
    pub(crate) fn fixture(aria2_rpc_url: String) -> Self {
        AppState {
            config: Arc::new(Config {
                aria2_rpc_url: aria2_rpc_url.clone(),
                ..Config::fixture()
            }),
            aria2: Aria2::new(aria2_rpc_url, reqwest::Client::new()),
            sessions: Sessions::new(),
            activity: Activity::new(),
            poll: Poll::new(),
            clearing: Clearing::default(),
            adding: Adding::default(),
            preexisting: Preexisting::default(),
            http: reqwest::Client::new(),
        }
    }
}
