//! Adding a magnet: starting it in aria2, then following it to the download it
//! becomes and narrowing that to the files worth keeping.
//!
//! [`start`] is what a request waits for: one `addUri`, which aria2 answers at
//! once. Everything after it, resolving the metadata, choosing the files and
//! releasing the download, can take minutes and runs detached ([`finish`]),
//! so its outcome reaches a person through the live list and the download's
//! activity log, never through the response. `resume` calls [`finish`] too, for
//! an add a restart interrupted.

use std::collections::HashSet;
use std::time::Duration;

use crate::aria2::Status;
use crate::error::{AppError, AppResult};
use crate::filter;
use crate::state::AppState;

/// Whether `s` is a magnet link: the one kind of link this takes. A file
/// upload is a different interface for a case that does not come up on a
/// phone.
pub fn is_magnet(s: &str) -> bool {
    s.starts_with("magnet:?")
}

fn is_already_registered(message: &str) -> bool {
    message.contains("is already registered")
}

fn magnet_infohash(magnet: &str) -> Option<&str> {
    let after = magnet.split_once("btih:")?.1;
    Some(after.split('&').next().unwrap_or(after))
}

/// Why aria2 would not take a magnet that passed validation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// aria2 already holds this infohash.
    AlreadyListed,
    /// aria2 did not answer, or answered with something else.
    Unreachable,
}

/// Starts `magnet` resolving into `dir` and hands the rest to [`finish`].
///
/// No `bt-metadata-only`: that option means "fetch the metadata and stop", so
/// aria2 never spawns the followed download and every add would run out the
/// clock. Plain `follow-torrent` (`mem`) splits into the parent-resolves,
/// child-downloads shape [`finish`] depends on.
pub async fn start(state: &AppState, sub: &str, magnet: &str, dir: &str) -> Result<(), Refused> {
    let metadata_gid = match state
        .aria2
        .add_uri(
            magnet,
            serde_json::json!({"dir": dir, "follow-torrent": "mem"}),
        )
        .await
    {
        Ok(gid) => gid,
        // Not a server fault: this infohash is already known to aria2, either
        // downloading already or a metadata pass an earlier attempt left
        // registered. aria2's error does not say which, and clearing it here
        // could cancel a real download, so the person is told and the list on
        // the page answers which it is.
        Err(AppError::Upstream(msg)) if is_already_registered(&msg) => {
            return Err(Refused::AlreadyListed);
        }
        Err(e) => {
            tracing::error!(error = %e, "add: aria2 refused the magnet");
            return Err(Refused::Unreachable);
        }
    };

    state
        .activity
        .record(&metadata_gid, format!("added — destination {dir}"));
    state
        .activity
        .record(&metadata_gid, "looking for peers who have the torrent file");

    let state = state.clone();
    let sub = sub.to_string();
    let magnet = magnet.to_string();
    let dir = dir.to_string();
    tokio::spawn(async move {
        if let Err(e) = finish(&state, &sub, &dir, &metadata_gid).await {
            tracing::error!(
                gid = %metadata_gid,
                infohash = magnet_infohash(&magnet).unwrap_or("unknown"),
                error = %e,
                "add: failed after the request was already accepted"
            );
            // Nobody reads a pod's log to find out why a row has said
            // "finding peers" for ten minutes; the row says now.
            state
                .activity
                .record(&metadata_gid, format!("failed — {e}"));
        }
    });
    Ok(())
}

/// How long to wait for a magnet to resolve before giving up. A well-seeded
/// torrent resolved in about 24 seconds against the real compose stack, and
/// nobody is waiting on this request, so a sparse swarm is given real time.
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(600);

/// Selects and starts the real download by following the metadata-resolving
/// gid to the download aria2 spawned from it, rather than issuing a second
/// `addUri` for the same magnet.
///
/// A magnet's own `files` describes the resolving gid itself: one entry,
/// whatever the torrent holds. `follow-torrent` spawns a second download from
/// the resolved metadata and links the two (`followedBy` on this one), and it
/// is that second gid's `files` that lists what the torrent contains, so the
/// selection is read there. Applying it through `changeOption` keeps one
/// download for the magnet from start to finish, under one gid, with no
/// infohash collision to resolve.
///
/// `--pause-metadata`, set on the aria2 process, creates the followed download
/// paused, so the selection is in place before a content byte arrives and this
/// only releases it at the end. Its absence is not assumed: the child's own
/// status decides whether there is anything to release.
pub async fn finish(state: &AppState, sub: &str, dir: &str, metadata_gid: &str) -> AppResult<()> {
    // Held for the whole sequence, so the reconciler in `resume`, which
    // recognises an unfinished add by exactly the state this is in the middle
    // of, leaves this one alone. Dropped on every exit.
    let _adding = state.adding.claim(metadata_gid);

    let child_gid = follow(state, metadata_gid).await?;

    // An adopted add is already the download it would have been followed to
    // (see `follow`), so there is no second gid to link a history to and no
    // second claim to take.
    let followed = child_gid != metadata_gid;

    // The history so far was written under the gid that resolved the magnet,
    // and that gid is about to leave every list the page reads. From here both
    // names reach one log. Claimed before the first await that could let the
    // reconciler see the child.
    let _adding_child = followed.then(|| {
        state.activity.link(metadata_gid, &child_gid);
        state.adding.claim(&child_gid)
    });
    state
        .activity
        .record(&child_gid, "torrent file arrived — choosing files");

    // The resolved magnet's own result stays in aria2's stopped list until
    // something discards it. Best effort: a leftover one is filtered out of
    // every list anyway.
    if followed && let Err(e) = state.aria2.remove_download_result(metadata_gid).await {
        tracing::debug!(gid = %metadata_gid, error = %e, "add: could not discard the resolved magnet");
    }

    // aria2 creates the child only once it has parsed a whole torrent, so a
    // paused child already answers with every file in it.
    let child = state.aria2.tell_status(&child_gid).await?;
    let paused = child.status == Status::Paused;
    let name = child.display_name().to_string();
    let files = child.files;

    // A download handed over running may have written its own files already,
    // so only a paused one is recorded; without a record, cancelling falls
    // back to the cautious rule.
    if paused {
        record_preexisting(state, &child_gid, &files).await;
    }
    if files.is_empty() {
        return Err(AppError::Upstream(
            "aria2 followed the metadata pass but the download it created has no files".to_string(),
        ));
    }

    let indices: Vec<String> = files
        .iter()
        .filter(|f| !filter::is_garbage(&f.path))
        .map(|f| f.index.to_string())
        .collect();
    if indices.is_empty() {
        // Left in place it would be paused with no selection, which `resume`
        // adopts as an interrupted add.
        if let Err(e) = state.aria2.remove(&child_gid).await {
            tracing::warn!(gid = %child_gid, error = %e, "add: could not remove an all-garbage torrent");
        }
        let _ = state.aria2.remove_download_result(&child_gid).await;
        // No row is left to carry the reason, so it is announced like any
        // other failed download.
        crate::notify::failed(
            state,
            &name,
            "every file is garbage, so it was not downloaded",
        )
        .await;
        return Err(AppError::BadRequest(
            "every file in this torrent is garbage, so it was removed".to_string(),
        ));
    }

    // What the filter kept out, and what that saved: the only record that a
    // skipped file never reached the disk, and the audit line for a write
    // onto the media library.
    let (skipped, skipped_bytes) = files
        .iter()
        .filter(|f| filter::is_garbage(&f.path))
        .fold((0u32, 0u64), |(n, b), f| (n + 1, b + f.length));
    tracing::info!(
        sub = %sub,
        magnet_files = files.len(),
        wanted = indices.len(),
        skipped,
        skipped_bytes,
        %dir,
        "starting download"
    );
    state.activity.record(
        &child_gid,
        format!(
            "keeping {} of {} files, skipping {skipped} ({})",
            indices.len(),
            files.len(),
            crate::format::bytes(skipped_bytes)
        ),
    );

    state
        .aria2
        .change_option(
            &child_gid,
            serde_json::json!({"dir": dir, "select-file": indices.join(",")}),
        )
        .await?;

    if paused {
        unpause_settled(state, &child_gid).await?;
    }
    state.activity.record(&child_gid, "downloading");

    Ok(())
}

/// Record which of a paused download's listed files are already on disk.
/// Called before aria2 has opened any of them, so whatever exists was in the
/// library first: see `state::Preexisting`.
async fn record_preexisting(state: &AppState, gid: &str, files: &[crate::aria2::File]) {
    let mut existing = HashSet::new();
    for f in files {
        if tokio::fs::symlink_metadata(&f.path).await.is_ok() {
            existing.insert(std::path::PathBuf::from(&f.path));
        }
    }
    state.preexisting.record(gid, existing).await;
}

/// The download a selection has to be applied to, given the gid an add is
/// known by.
///
/// Normally another gid: a magnet resolves under its own, and
/// `follow-torrent` spawns the real download beside it. But `resume` adopts an
/// add by whatever aria2 holds, and after a restart that is often the spawned
/// download itself (`--bt-load-saved-metadata` reads the torrent off the disk
/// at start-up), so there is no `followedBy` that will ever appear. The two are
/// told apart by `is_metadata`: aria2 gives an unresolved magnet no torrent
/// name and a single file called `[METADATA]<dn>`.
async fn follow(state: &AppState, gid: &str) -> AppResult<String> {
    let deadline = tokio::time::Instant::now() + RESOLVE_TIMEOUT;
    loop {
        let status = state.aria2.tell_status(gid).await?;
        if status.status == Status::Error {
            return Err(AppError::Upstream(
                status
                    .error_message
                    .unwrap_or_else(|| "metadata fetch failed".to_string()),
            ));
        }
        if let Some(child) = status.followed_by.first() {
            return Ok(child.clone());
        }
        if !status.is_metadata() && !status.files.is_empty() {
            return Ok(gid.to_string());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(AppError::Upstream(format!(
                "the magnet did not resolve within {}s",
                RESOLVE_TIMEOUT.as_secs()
            )));
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// How long to keep offering aria2 the chance to accept the unpause. Generous,
/// because the only thing waiting on it is a download that is already stopped.
const UNPAUSE_TIMEOUT: Duration = Duration::from_secs(15);

/// Releases a paused download, allowing for aria2 not being ready to.
///
/// A pause is a request rather than an act: the group keeps running until it
/// has closed what it had open, and until then aria2 answers `GID#… cannot be
/// unpaused now`. Giving up on the first refusal leaves a paused download with
/// nothing to start it.
async fn unpause_settled(state: &AppState, gid: &str) -> AppResult<()> {
    let deadline = tokio::time::Instant::now() + UNPAUSE_TIMEOUT;
    loop {
        match state.aria2.unpause(gid).await {
            Ok(()) => return Ok(()),
            Err(e) if tokio::time::Instant::now() >= deadline => return Err(e),
            Err(e) => {
                tracing::debug!(%gid, error = %e, "add: aria2 will not unpause yet, retrying");
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use axum::Json;
    use axum::extract::State;
    use axum::routing::post;

    use super::*;
    use crate::config::{Config, Root};

    /// What the mock answers for `aria2.tellStatus` on the metadata gid.
    #[derive(Clone)]
    pub(crate) enum MetadataOutcome {
        /// No `followedBy` ever appears.
        NeverResolves,
        /// The metadata resolves into `child_gid`, listing `child_files`.
        ResolvesWithChild {
            child_gid: String,
            child_files: serde_json::Value,
        },
    }

    #[derive(Clone)]
    struct MockAria2 {
        calls: Arc<Mutex<Vec<String>>>,
        change_option_calls: Arc<Mutex<Vec<serde_json::Value>>>,
        metadata_add_result: Arc<serde_json::Value>,
        metadata_outcome: MetadataOutcome,
        /// How aria2 hands the followed download over: "paused" under
        /// `--pause-metadata`, "active" without it.
        child_status: &'static str,
        /// `unpause` calls refused before one is accepted.
        unpause_refusals: Arc<Mutex<u32>>,
        /// Every URI `addUri` has been given.
        added_uris: Arc<Mutex<Vec<String>>>,
    }

    fn download_result(gid: &str, status: &str, files: serde_json::Value) -> serde_json::Value {
        serde_json::json!({"result": {
            "gid": gid,
            "status": status,
            "totalLength": "100",
            "completedLength": "0",
            "downloadSpeed": "0",
            "uploadSpeed": "0",
            "connections": "0",
            "dir": "/mnt/kontent/movies",
            "files": files,
        }})
    }

    async fn rpc(
        State(mock): State<MockAria2>,
        Json(body): Json<serde_json::Value>,
    ) -> Json<serde_json::Value> {
        let method = body["method"].as_str().unwrap_or_default().to_string();
        mock.calls.lock().unwrap().push(method.clone());
        let seen_before = method == "aria2.addUri" && {
            let uri = body["params"][0][0]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let mut added = mock.added_uris.lock().unwrap();
            let seen = added.contains(&uri);
            added.push(uri);
            seen
        };
        let response = match method.as_str() {
            // One `addUri` for a magnet's whole life: a second is the
            // duplicate-infohash bug back.
            "aria2.addUri" if seen_before => {
                panic!("a second addUri means the duplicate-infohash bug is back")
            }
            "aria2.addUri" => (*mock.metadata_add_result).clone(),
            "aria2.pause" => panic!("finish must not pause anything itself"),
            "aria2.unpause" => {
                let mut refusals = mock.unpause_refusals.lock().unwrap();
                if *refusals > 0 {
                    *refusals -= 1;
                    serde_json::json!({"error": {
                        "code": 1,
                        "message": "GID#child-gid cannot be unpaused now",
                    }})
                } else {
                    serde_json::json!({"result": "OK"})
                }
            }
            "aria2.tellStatus" => {
                let gid = body["params"][0].as_str().unwrap_or_default();
                match (&mock.metadata_outcome, gid) {
                    (MetadataOutcome::NeverResolves, _) => {
                        download_result("metadata-gid", "active", serde_json::json!([]))
                    }
                    (
                        MetadataOutcome::ResolvesWithChild {
                            child_gid,
                            child_files,
                        },
                        g,
                    ) if g == child_gid => {
                        download_result(child_gid, mock.child_status, child_files.clone())
                    }
                    (MetadataOutcome::ResolvesWithChild { child_gid, .. }, _) => {
                        let mut result =
                            download_result("metadata-gid", "complete", serde_json::json!([]));
                        result["result"]["followedBy"] = serde_json::json!([child_gid.clone()]);
                        result
                    }
                }
            }
            "aria2.changeOption" => {
                mock.change_option_calls.lock().unwrap().push(body.clone());
                serde_json::json!({"result": "OK"})
            }
            "aria2.forceRemove" | "aria2.removeDownloadResult" => {
                serde_json::json!({"result": "OK"})
            }
            other => panic!("unexpected aria2 method in test: {other}"),
        };
        let mut envelope = serde_json::json!({"jsonrpc": "2.0", "id": "mariastew"});
        envelope
            .as_object_mut()
            .unwrap()
            .extend(response.as_object().unwrap().clone());
        Json(envelope)
    }

    pub(crate) struct Mock {
        pub(crate) url: String,
        pub(crate) calls: Arc<Mutex<Vec<String>>>,
        change_option_calls: Arc<Mutex<Vec<serde_json::Value>>>,
    }

    pub(crate) async fn mock_server(
        metadata_add_result: serde_json::Value,
        metadata_outcome: MetadataOutcome,
    ) -> Mock {
        mock_server_with(metadata_add_result, metadata_outcome, "paused", 0).await
    }

    async fn mock_server_with(
        metadata_add_result: serde_json::Value,
        metadata_outcome: MetadataOutcome,
        child_status: &'static str,
        unpause_refusals: u32,
    ) -> Mock {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let change_option_calls = Arc::new(Mutex::new(Vec::new()));
        let mock = MockAria2 {
            calls: calls.clone(),
            change_option_calls: change_option_calls.clone(),
            metadata_add_result: Arc::new(metadata_add_result),
            metadata_outcome,
            child_status,
            unpause_refusals: Arc::new(Mutex::new(unpause_refusals)),
            added_uris: Arc::new(Mutex::new(Vec::new())),
        };
        let app = axum::Router::new().route("/", post(rpc)).with_state(mock);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Mock {
            url: format!("http://{addr}/"),
            calls,
            change_option_calls,
        }
    }

    /// The service against `url`, with one root at `/mnt/kontent/movies`.
    pub(crate) fn state(url: String) -> AppState {
        let mut s = AppState::fixture(url);
        s.config = Arc::new(Config {
            aria2_rpc_url: s.config.aria2_rpc_url.clone(),
            roots: vec![Root {
                name: "movies".to_string(),
                path: std::path::PathBuf::from("/mnt/kontent/movies"),
            }],
            ..Config::fixture()
        });
        s
    }

    const MAGNET: &str = "magnet:?xt=urn:btih:abc";
    const DIR: &str = "/mnt/kontent/movies";

    async fn wait_for_calls(calls: &Mutex<Vec<String>>, expected: usize) {
        for _ in 0..200 {
            if calls.lock().unwrap().len() >= expected {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!(
            "timed out waiting for {expected} calls, saw {:?}",
            calls.lock().unwrap()
        );
    }

    fn resolves(child_files: serde_json::Value) -> MetadataOutcome {
        MetadataOutcome::ResolvesWithChild {
            child_gid: "child-gid".to_string(),
            child_files,
        }
    }

    /// The request waits for one `addUri` and nothing else: a browser does
    /// not hold a POST open for a metadata poll (production saw
    /// `NS_BINDING_ABORTED` around two seconds).
    #[tokio::test]
    async fn starting_returns_before_the_metadata_resolves() {
        let mock = mock_server(
            serde_json::json!({"result": "metadata-gid"}),
            MetadataOutcome::NeverResolves,
        )
        .await;
        let state = state(mock.url);
        tokio::time::timeout(
            Duration::from_millis(500),
            start(&state, "tester", MAGNET, DIR),
        )
        .await
        .expect("start must return promptly")
        .expect("aria2 took the magnet");
    }

    /// The selection is read from the download `follow-torrent` spawned, not
    /// the metadata gid's own single-entry list. The video at index 2 and a
    /// nested `Other/` entry are the case that went wrong before.
    #[tokio::test]
    async fn the_selection_is_read_from_the_followed_downloads_file_list() {
        let child_files = serde_json::json!([
            {"index": "1", "path": "/mnt/kontent/movies/Nightfall Harbour (1994) [1080p]/PACKGRP.nfo", "length": "473", "selected": "true"},
            {"index": "2", "path": "/mnt/kontent/movies/Nightfall Harbour (1994) [1080p]/Nightfall.Harbour.1994.1080p.BrRip.x264.QUARRY.mp4", "length": "1932735283", "selected": "true"},
            {"index": "3", "path": "/mnt/kontent/movies/Nightfall Harbour (1994) [1080p]/Other/COVERART.jpg", "length": "53226", "selected": "true"},
        ]);
        let mock = mock_server(
            serde_json::json!({"result": "metadata-gid"}),
            resolves(child_files),
        )
        .await;
        let state = state(mock.url);
        start(&state, "tester", MAGNET, DIR).await.unwrap();

        wait_for_calls(&mock.calls, 6).await;
        assert_eq!(
            mock.calls.lock().unwrap().as_slice(),
            [
                "aria2.addUri",
                "aria2.tellStatus",
                "aria2.removeDownloadResult",
                "aria2.tellStatus",
                "aria2.changeOption",
                "aria2.unpause",
            ]
        );
        let change_option_calls = mock.change_option_calls.lock().unwrap();
        assert_eq!(change_option_calls.len(), 1);
        let params = &change_option_calls[0]["params"];
        assert_eq!(params[0], "child-gid");
        assert_eq!(params[1]["select-file"], "2");
        assert_eq!(params[1]["dir"], DIR);
    }

    /// The account of an add is written under the gid that resolves the
    /// magnet, which leaves every list the moment the real download exists.
    #[tokio::test]
    async fn the_history_of_an_add_follows_the_download_it_produced() {
        let child_files = serde_json::json!([
            {"index": "1", "path": "/mnt/kontent/movies/movie.mkv", "length": "100", "selected": "true"},
            {"index": "2", "path": "/mnt/kontent/movies/PACKGRP.nfo", "length": "473", "selected": "true"},
        ]);
        let mock = mock_server(
            serde_json::json!({"result": "metadata-gid"}),
            resolves(child_files),
        )
        .await;
        let state = state(mock.url);
        start(&state, "tester", MAGNET, DIR).await.unwrap();
        wait_for_calls(&mock.calls, 6).await;

        let messages: Vec<String> = state
            .activity
            .entries("child-gid")
            .into_iter()
            .map(|e| e.message)
            .collect();
        assert!(
            messages
                .iter()
                .any(|m| m.contains("added — destination /mnt/kontent/movies")),
            "the child lost what was recorded before it existed: {messages:?}"
        );
        assert!(
            messages
                .iter()
                .any(|m| m.contains("keeping 1 of 2 files, skipping 1")),
            "the filter's decision is not in the log: {messages:?}"
        );
    }

    /// An aria2 started without `--pause-metadata` hands the child over
    /// running, and unpausing that is refused, so the child's own status
    /// decides.
    #[tokio::test]
    async fn a_child_aria2_handed_over_running_is_not_unpaused() {
        let child_files = serde_json::json!([
            {"index": "1", "path": "/mnt/kontent/movies/movie.mkv", "length": "100", "selected": "true"},
        ]);
        let mock = mock_server_with(
            serde_json::json!({"result": "metadata-gid"}),
            resolves(child_files),
            "active",
            0,
        )
        .await;
        let state = state(mock.url);
        start(&state, "tester", MAGNET, DIR).await.unwrap();

        wait_for_calls(&mock.calls, 5).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            mock.calls.lock().unwrap().as_slice(),
            [
                "aria2.addUri",
                "aria2.tellStatus",
                "aria2.removeDownloadResult",
                "aria2.tellStatus",
                "aria2.changeOption",
            ],
            "a child that was never paused must not be unpaused"
        );
    }

    /// aria2 refuses to unpause a group until it has finished stopping, so a
    /// single attempt loses a race and leaves the download stopped for good.
    #[tokio::test]
    async fn an_unpause_aria2_is_not_ready_for_is_asked_again() {
        let child_files = serde_json::json!([
            {"index": "1", "path": "/mnt/kontent/movies/movie.mkv", "length": "100", "selected": "true"},
        ]);
        let mock = mock_server_with(
            serde_json::json!({"result": "metadata-gid"}),
            resolves(child_files),
            "paused",
            2,
        )
        .await;
        let state = state(mock.url);
        start(&state, "tester", MAGNET, DIR).await.unwrap();

        wait_for_calls(&mock.calls, 8).await;
        assert_eq!(
            mock.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|m| *m == "aria2.unpause")
                .count(),
            3,
            "the add gave up on its own pause"
        );
    }

    /// An all-garbage torrent cannot be narrowed to anything. Left paused with
    /// no selection, `resume` would adopt it, so it is removed instead.
    #[tokio::test]
    async fn an_all_garbage_torrent_is_removed_rather_than_left_to_be_adopted() {
        let child_files = serde_json::json!([
            {"index": "1", "path": "/mnt/kontent/movies/PACKGRP.nfo", "length": "473", "selected": "true"},
            {"index": "2", "path": "/mnt/kontent/movies/cover.jpg", "length": "900", "selected": "true"},
        ]);
        let mock = mock_server(
            serde_json::json!({"result": "metadata-gid"}),
            resolves(child_files),
        )
        .await;
        let state = state(mock.url);
        start(&state, "tester", MAGNET, DIR).await.unwrap();

        wait_for_calls(&mock.calls, 6).await;
        let calls = mock.calls.lock().unwrap();
        assert!(
            calls.iter().any(|m| m == "aria2.forceRemove"),
            "the torrent was left for resume to adopt: {calls:?}"
        );
        assert!(
            !calls
                .iter()
                .any(|m| m == "aria2.changeOption" || m == "aria2.unpause"),
            "an all-garbage torrent was narrowed or released: {calls:?}"
        );
    }

    #[tokio::test]
    async fn an_infohash_aria2_already_holds_is_refused_as_listed() {
        let mock = mock_server(
            serde_json::json!({"error": {"code": 1, "message": "InfoHash abc is already registered."}}),
            MetadataOutcome::NeverResolves,
        )
        .await;
        let state = state(mock.url);
        assert_eq!(
            start(&state, "tester", MAGNET, DIR).await,
            Err(Refused::AlreadyListed)
        );
    }

    #[test]
    fn a_magnet_link_is_accepted_and_anything_else_is_not() {
        assert!(is_magnet("magnet:?xt=urn:btih:abc123"));
        assert!(!is_magnet("http://example.com/foo.torrent"));
        assert!(!is_magnet("foo.torrent"));
        assert!(!is_magnet(""));
    }

    #[test]
    fn already_registered_is_recognised_by_aria2s_own_wording() {
        assert!(is_already_registered(
            "InfoHash abcd1234 is already registered."
        ));
        assert!(!is_already_registered("connection refused"));
    }

    #[test]
    fn magnet_infohash_reads_the_btih_parameter_and_stops_at_the_next_one() {
        assert_eq!(
            magnet_infohash("magnet:?xt=urn:btih:ABCD1234&dn=Some.Show"),
            Some("ABCD1234")
        );
        assert_eq!(magnet_infohash("magnet:?dn=no-hash-here"), None);
    }
}
