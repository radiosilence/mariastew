//! Taking a download out of aria2, and deleting from the media library what it
//! should not leave behind.
//!
//! Every deletion goes through [`remove_contained`], which unlinks only a
//! regular file inside a root reached through no symlink. Two callers delete:
//! [`remove`], when a person clears an unfinished download, and the garbage
//! sweep in `notify`, when a finished one turns out to have written files the
//! selection could not stop.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::aria2::Download;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// How a removal ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Removed {
    /// aria2 no longer lists it.
    Gone,
    /// aria2 still lists it after being told to drop it.
    Held,
    /// Another request is already removing it.
    AlreadyClearing,
}

/// Two behaviours behind one button, decided by whether the download
/// completed. In progress, the partial data goes with it: a half-downloaded
/// episode sitting in the library is the mess this exists to avoid. Finished
/// and seeding, the files stay: that is stopping an upload, not undoing a
/// download.
///
/// The download is marked clearing for every open page (`state::Clearing`)
/// while aria2 is asked to let go, and the call returns once it has, so the
/// caller's answer is the outcome rather than a guess at it. The work runs in
/// a task that is then awaited: a browser that gives up on the response drops
/// this future, and the removal has to outlive that, since aria2 has already
/// been told to stop by then.
pub async fn remove(state: &AppState, who: &str, gid: &str) -> AppResult<Removed> {
    let status = state.aria2.tell_status(gid).await?;
    // The only request that destroys anything, so it says who asked and which
    // of the two meanings applied before it does.
    tracing::info!(
        sub = %who,
        gid = %gid,
        name = %status.name(),
        finished = status.is_finished(),
        "removing download"
    );
    if !state.clearing.begin(gid) {
        return Ok(Removed::AlreadyClearing);
    }
    state.activity.record(gid, "clearing");
    let task = tokio::spawn({
        let state = state.clone();
        let gid = gid.to_string();
        async move { clear(&state, &gid, &status).await }
    });
    match task.await {
        Ok(true) => Ok(Removed::Gone),
        Ok(false) => Ok(Removed::Held),
        // Only a panic reaches here, and `clear` lifts the mark on every path
        // it returns from, so this is the one place left that could strand a
        // row as permanently clearing.
        Err(e) => {
            state.clearing.end(gid);
            Err(AppError::Internal(anyhow::anyhow!(
                "clearing {gid} panicked: {e}"
            )))
        }
    }
}

/// How long to keep asking aria2 whether it has let go, and how often.
/// `forceRemove` stops the download outright, but it reaches the stopped list
/// a moment after the call returns and `removeDownloadResult` refuses it until
/// it does, so one attempt is not enough and a second usually is.
const CLEAR_ATTEMPTS: u32 = 10;
const CLEAR_POLL: Duration = Duration::from_millis(100);

/// aria2's answer for a gid it holds in no list at all, in either of its two
/// wordings: one once the gid itself has been freed, one while it lingers.
fn is_not_found(message: &str) -> bool {
    message.ends_with("is not found") || message.starts_with("No such download")
}

/// Make aria2 forget a download, then delete whatever it should not have left
/// behind.
///
/// The two removal calls apply to different downloads and each refuses the
/// other's: `forceRemove` stops one that is still running,
/// `removeDownloadResult` discards one that has already stopped, which every
/// errored row and every torrent aria2 is no longer seeding is. So the outcome
/// is decided by whether aria2 still answers `tellStatus` for the gid at all,
/// asked after each discard attempt.
async fn clear(state: &AppState, gid: &str, status: &Download) -> bool {
    if !status.is_stopped()
        && let Err(e) = state.aria2.remove(gid).await
    {
        tracing::warn!(gid = %gid, error = %e, "could not stop download, discarding it anyway");
    }

    let mut gone = false;
    for attempt in 0..CLEAR_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(CLEAR_POLL).await;
        }
        let _ = state.aria2.remove_download_result(gid).await;
        match state.aria2.tell_status(gid).await {
            Ok(_) => {}
            Err(AppError::Upstream(msg)) if is_not_found(&msg) => gone = true,
            Err(e) => tracing::warn!(gid = %gid, error = %e, "clear: aria2 status failed"),
        }
        if gone {
            break;
        }
    }

    if gone {
        if !status.is_finished() {
            delete_partial(state, status).await;
        }
        // Nothing will draw this row again, so its history has nothing left
        // to explain: the one place a log goes before the cap takes it.
        state.activity.forget(gid);
        state.preexisting.forget(gid);
    } else {
        tracing::warn!(gid = %gid, "aria2 still lists the download after removal");
        state.activity.record(gid, "aria2 would not let go of it");
    }
    state.clearing.end(gid);
    gone
}

/// Delete what a cancelled torrent wrote: its files, their `.aria2` control
/// files, the torrent's own control file, and the directories that leaves
/// empty. Never a file that was in the library before the download started
/// (see `state::Preexisting`), nor a deselected one aria2 holds no bytes for.
/// For a download with no such record, one added before this process started,
/// only selected files aria2 has not completed go, since a complete one may be
/// a pre-existing file aria2 verified. Failures are logged, not fatal: the
/// download is already out of the queue either way.
async fn delete_partial(state: &AppState, status: &Download) {
    let known = state.preexisting.known(&status.gid);
    let mut deleted = Vec::new();
    for f in &status.files {
        let wrote = match &known {
            // A deselected path aria2 holds no bytes for is not its own:
            // something else may have put a file there since the add.
            Some(existing) => {
                !existing.contains(Path::new(&f.path)) && (f.selected || f.completed_length > 0)
            }
            None => f.selected && f.completed_length < f.length,
        };
        if wrote && remove_contained(state, &f.path).await {
            let _ = tokio::fs::remove_file(format!("{}.aria2", f.path)).await;
            deleted.push(PathBuf::from(&f.path));
        }
    }
    // A multi-file torrent's control file sits beside its tree, named for the
    // torrent, rather than beside any one file. An unresolved magnet has
    // written none.
    if !status.is_metadata() {
        let control = Path::new(&status.dir).join(format!("{}.aria2", status.name()));
        remove_contained(state, &control.to_string_lossy()).await;
    }
    remove_emptied_dirs(state, Path::new(&status.dir), &deleted).await;
}

/// Unlink `path` if it is a regular file inside a root, reached through no
/// symlink: the one gate every deletion from the library goes through. A path
/// that is not there is skipped quietly (every deselected file that never
/// arrived is one), and one that resolves outside the roots, or through a
/// link, is refused out loud.
pub async fn remove_contained(state: &AppState, path: &str) -> bool {
    if !matches!(tokio::fs::symlink_metadata(path).await, Ok(m) if m.is_file()) {
        return false;
    }
    if state.config.resolve_unlinked(path).await.is_none() {
        tracing::warn!(%path, "refusing to delete a path outside the roots or through a link");
        return false;
    }
    match tokio::fs::remove_file(path).await {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(%path, error = %e, "failed to delete a file");
            false
        }
    }
}

/// Remove the directories between each deleted file and `dir` that are now
/// empty, deepest first. `dir` is where the download was added and is never
/// removed; `remove_dir` refusing a non-empty directory keeps anything with
/// other contents. Nothing is removed for a `dir` that is not an absolute
/// path, since every path starts with an empty one.
pub async fn remove_emptied_dirs(state: &AppState, dir: &Path, deleted: &[PathBuf]) {
    if !dir.is_absolute() {
        return;
    }
    let mut dirs: Vec<_> = deleted
        .iter()
        .flat_map(|path| {
            path.ancestors()
                .skip(1)
                .take_while(|ancestor| *ancestor != dir && ancestor.starts_with(dir))
        })
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    for d in dirs {
        if state
            .config
            .resolve_unlinked(&d.to_string_lossy())
            .await
            .is_some()
        {
            let _ = tokio::fs::remove_dir(d).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::Json;
    use axum::extract::State;
    use axum::routing::post;

    use super::*;
    use crate::config::{Config, Root};

    fn state_with_root(url: String, root: Option<PathBuf>) -> AppState {
        let mut state = AppState::fixture(url);
        if let Some(root) = root {
            state.config = Arc::new(Config {
                roots: vec![Root {
                    name: "tv".to_string(),
                    path: root,
                }],
                ..Config::fixture()
            });
        }
        state
    }

    /// A cancelled torrent takes every directory it made with it, not just the
    /// one its files sat in, and leaves the destination and anything that was
    /// already there alone.
    #[tokio::test]
    async fn cancelling_removes_the_whole_tree_the_torrent_made_and_nothing_else() {
        let root = std::env::temp_dir().join("mariastew-delete-partial");
        let _ = tokio::fs::remove_dir_all(&root).await;
        let episode = root.join("Show/Season 1/ep01.mkv");
        let neighbour = root.join("Other/keep.mkv");
        let control = root.join("Show.aria2");
        let already_there = root.join("Show/Season 1/ep00.mkv");
        let dropped_in = root.join("Show/poster.jpg");
        for f in [&episode, &neighbour, &control, &already_there, &dropped_in] {
            tokio::fs::create_dir_all(f.parent().unwrap())
                .await
                .unwrap();
            tokio::fs::write(f, b"bytes").await.unwrap();
        }
        let state = state_with_root(String::new(), Some(root.clone()));
        let file = |path: &Path| crate::aria2::File {
            index: 1,
            path: path.to_string_lossy().into_owned(),
            length: 5,
            completed_length: 1,
            selected: true,
        };
        let download = Download {
            dir: root.to_string_lossy().into_owned(),
            files: vec![
                file(&episode),
                file(&root.join("Show/never-arrived.nfo")),
                crate::aria2::File {
                    selected: false,
                    completed_length: 0,
                    ..file(&dropped_in)
                },
                file(&already_there),
            ],
            bittorrent_name: Some("Show".to_string()),
            ..Download::fixture()
        };

        state
            .preexisting
            .record(&download.gid, HashSet::from([already_there.clone()]));
        delete_partial(&state, &download).await;

        assert!(
            dropped_in.exists(),
            "a file aria2 never wrote, created after the add, was deleted"
        );
        assert!(
            already_there.exists(),
            "a file already in the library before the add was deleted"
        );
        assert!(!episode.exists(), "the torrent's own file was left behind");
        assert!(
            !control.exists(),
            "the torrent's control file was left behind"
        );
        assert!(
            neighbour.exists(),
            "something the torrent did not make was removed"
        );
        assert!(root.exists(), "the destination itself was removed");
        let _ = tokio::fs::remove_dir_all(&root).await;
    }

    /// Without a record of what was there first, a complete file may be one
    /// aria2 verified rather than fetched, so only incomplete ones go.
    #[tokio::test]
    async fn cancelling_without_a_record_keeps_complete_files() {
        let root = std::env::temp_dir().join("mariastew-delete-unrecorded");
        let _ = tokio::fs::remove_dir_all(&root).await;
        let complete = root.join("Show/ep01.mkv");
        let partial = root.join("Show/ep02.mkv");
        for f in [&complete, &partial] {
            tokio::fs::create_dir_all(f.parent().unwrap())
                .await
                .unwrap();
            tokio::fs::write(f, b"bytes").await.unwrap();
        }
        let state = state_with_root(String::new(), Some(root.clone()));
        let file = |path: &Path, completed_length| crate::aria2::File {
            index: 1,
            path: path.to_string_lossy().into_owned(),
            length: 5,
            completed_length,
            selected: true,
        };
        let download = Download {
            dir: root.to_string_lossy().into_owned(),
            files: vec![file(&complete, 5), file(&partial, 2)],
            ..Download::fixture()
        };

        delete_partial(&state, &download).await;

        assert!(
            complete.exists(),
            "a complete file with no record was deleted"
        );
        assert!(!partial.exists(), "an incomplete file was left behind");
        let _ = tokio::fs::remove_dir_all(&root).await;
    }

    #[test]
    fn not_found_is_recognised_by_aria2s_own_wording() {
        assert!(is_not_found("GID 2089b05ecca3d829 is not found"));
        assert!(is_not_found("No such download for GID#2089b05ecca3d829"));
        assert!(!is_not_found("connection refused"));
    }

    /// An aria2 that refuses the removal calls the way the real one does:
    /// `remove` only for a download it is still running, and
    /// `removeDownloadResult` only for one it has already stopped, and it does
    /// not move between those the instant it is asked. The mock in
    /// `scripts/mock-aria2.ts` deletes on any of them and so could never show
    /// this.
    #[derive(Clone)]
    struct MockAria2 {
        /// `None` once aria2 has forgotten it.
        status: Arc<Mutex<Option<&'static str>>>,
        /// `removeDownloadResult` calls to refuse before it takes.
        discard_refusals: Arc<Mutex<u32>>,
        calls: Arc<Mutex<Vec<String>>>,
    }

    fn error(message: &str) -> serde_json::Value {
        serde_json::json!({"error": {"code": 1, "message": message}})
    }

    fn download(gid: &str, status: &str) -> serde_json::Value {
        serde_json::json!({
            "gid": gid,
            "status": status,
            "totalLength": "100",
            "completedLength": "100",
            "downloadSpeed": "0",
            "uploadSpeed": "0",
            "connections": "0",
            "dir": "/mnt/kontent/movies",
            "files": [],
        })
    }

    async fn rpc(
        State(mock): State<MockAria2>,
        Json(body): Json<serde_json::Value>,
    ) -> Json<serde_json::Value> {
        let method = body["method"].as_str().unwrap_or_default();
        let response = if method == "system.multicall" {
            let results: Vec<_> = body["params"][0]
                .as_array()
                .unwrap()
                .iter()
                .map(|call| {
                    let answer = answer(&mock, call["methodName"].as_str().unwrap());
                    match answer.get("result") {
                        Some(result) => serde_json::json!([result]),
                        None => serde_json::json!({"faultCode": 1, "faultString": answer["error"]["message"]}),
                    }
                })
                .collect();
            serde_json::json!({ "result": results })
        } else {
            answer(&mock, method)
        };
        let mut envelope = serde_json::json!({"jsonrpc": "2.0", "id": "mariastew"});
        envelope
            .as_object_mut()
            .unwrap()
            .extend(response.as_object().unwrap().clone());
        Json(envelope)
    }

    fn answer(mock: &MockAria2, method: &str) -> serde_json::Value {
        mock.calls.lock().unwrap().push(method.to_string());
        let mut status = mock.status.lock().unwrap();
        let stopped = |s: &str| matches!(s, "complete" | "error" | "removed");
        match method {
            "aria2.tellStatus" => match *status {
                Some(s) => serde_json::json!({"result": download("g1", s)}),
                None => error("GID g1 is not found"),
            },
            "aria2.tellActive" => match *status {
                Some("active") => serde_json::json!({"result": [download("g1", "active")]}),
                _ => serde_json::json!({"result": []}),
            },
            "aria2.tellWaiting" => serde_json::json!({"result": []}),
            "aria2.tellStopped" => match *status {
                Some(s) if stopped(s) => serde_json::json!({"result": [download("g1", s)]}),
                _ => serde_json::json!({"result": []}),
            },
            "aria2.remove" | "aria2.forceRemove" => match *status {
                Some(s) if !stopped(s) => {
                    *status = Some("removed");
                    serde_json::json!({"result": "g1"})
                }
                _ => error("GID#g1 cannot be removed now"),
            },
            "aria2.removeDownloadResult" => {
                let mut refusals = mock.discard_refusals.lock().unwrap();
                match *status {
                    Some(s) if stopped(s) && *refusals == 0 => {
                        *status = None;
                        serde_json::json!({"result": "OK"})
                    }
                    _ => {
                        *refusals = refusals.saturating_sub(1);
                        error("GID#g1 is not found")
                    }
                }
            }
            other => panic!("unexpected aria2 method in test: {other}"),
        }
    }

    pub(crate) struct Mock {
        pub(crate) url: String,
        pub(crate) status: Arc<Mutex<Option<&'static str>>>,
        pub(crate) calls: Arc<Mutex<Vec<String>>>,
    }

    pub(crate) async fn mock_server(status: &'static str, discard_refusals: u32) -> Mock {
        let mock = MockAria2 {
            status: Arc::new(Mutex::new(Some(status))),
            discard_refusals: Arc::new(Mutex::new(discard_refusals)),
            calls: Arc::new(Mutex::new(Vec::new())),
        };
        let app = axum::Router::new()
            .route("/", post(rpc))
            .with_state(mock.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Mock {
            url: format!("http://{addr}/"),
            status: mock.status,
            calls: mock.calls,
        }
    }

    /// aria2 refuses `remove` for a download it has already stopped (every
    /// errored row, and any finished torrent it is no longer seeding), so a
    /// removal that started there never reached `removeDownloadResult`, which
    /// would have worked.
    #[tokio::test]
    async fn a_download_aria2_has_already_stopped_is_still_discarded() {
        for status in ["complete", "error"] {
            let mock = mock_server(status, 0).await;
            let state = state_with_root(mock.url.clone(), None);
            assert_eq!(remove(&state, "tester", "g1").await.unwrap(), Removed::Gone);
            assert!(
                mock.status.lock().unwrap().is_none(),
                "a {status} download was left in aria2's list"
            );
        }
    }

    /// aria2 is not done when `remove` returns: the download has to reach the
    /// stopped list before `removeDownloadResult` will take it. One attempt
    /// left the corpse behind, where it was drawn as a live row.
    #[tokio::test]
    async fn a_removal_keeps_asking_until_aria2_has_actually_let_go() {
        let mock = mock_server("active", 3).await;
        let state = state_with_root(mock.url.clone(), None);
        state.activity.record("g1", "added");

        assert_eq!(remove(&state, "tester", "g1").await.unwrap(), Removed::Gone);

        assert!(!state.clearing.contains("g1"), "the mark was never lifted");
        assert!(
            state.activity.entries("g1").is_empty(),
            "a download nothing will draw again keeps no history"
        );
        let calls = mock.calls.lock().unwrap();
        assert!(
            calls
                .iter()
                .filter(|m| *m == "aria2.removeDownloadResult")
                .count()
                > 1,
            "one attempt is what left the corpse behind: {calls:?}"
        );
    }

    /// Stopping is `forceRemove`: the graceful `remove` announces to every
    /// tracker and waits for the peers to go, which is the wait the held
    /// response cannot afford and the press has already decided against.
    #[tokio::test]
    async fn stopping_a_running_download_does_not_wait_for_the_swarm() {
        let mock = mock_server("active", 0).await;
        let state = state_with_root(mock.url.clone(), None);
        remove(&state, "tester", "g1").await.unwrap();

        let calls = mock.calls.lock().unwrap();
        assert!(
            calls.iter().any(|m| m == "aria2.forceRemove"),
            "nothing forced the stop: {calls:?}"
        );
        assert!(
            !calls.iter().any(|m| m == "aria2.remove"),
            "the graceful call is the wait this avoids: {calls:?}"
        );
    }

    /// A removal that does not take is a row that is still there. The mark
    /// comes off either way: the controls it hid have to come back.
    #[tokio::test]
    async fn a_removal_aria2_refuses_says_so_and_hands_the_row_back() {
        let mock = mock_server("active", u32::MAX).await;
        let state = state_with_root(mock.url.clone(), None);
        assert_eq!(remove(&state, "tester", "g1").await.unwrap(), Removed::Held);
        assert!(!state.clearing.contains("g1"), "the mark was never lifted");
    }

    /// Every page open on the download reads the mark, not this request's
    /// answer, so it is set for the whole time the removal is in flight.
    #[tokio::test]
    async fn the_download_is_marked_clearing_while_the_removal_is_in_flight() {
        let mock = mock_server("active", u32::MAX).await;
        let state = state_with_root(mock.url.clone(), None);
        let in_flight = tokio::spawn({
            let state = state.clone();
            async move { remove(&state, "tester", "g1").await }
        });

        let mut marked = false;
        for _ in 0..200 {
            if state.clearing.contains("g1") {
                marked = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(marked, "the press was invisible until the removal finished");
        assert_eq!(
            remove(&state, "tester", "g1").await.unwrap(),
            Removed::AlreadyClearing,
            "a second press started a second removal"
        );
        in_flight.await.unwrap().unwrap();
    }
}
