//! Telegram, on the two events worth interrupting someone for.
//!
//! The point of leaving the house is not checking a web page to find out
//! whether a download finished. Finishing changes what you can do; failing is
//! arguably worth more, because nothing else will ever tell you. Starting is
//! not worth a message — it reports something you just did.
//!
//! Silent when unconfigured, the way the sing-box notifier is: absent
//! credentials skip rather than fail, so the service runs without Telegram at
//! all. Failures to send are logged and swallowed for the same reason — the
//! download succeeded, and a notification is not worth failing it over.
//!
//! The credentials and the message style are shared with
//! `packages/vpn/scripts/notify-singbox.ts`; the code is not. That is a Pulumi
//! command firing at deploy time when a hash changes, which is the wrong shape
//! for a runtime event.

use std::collections::HashMap;
use std::path::Path;

use crate::aria2::{Download, Health, Status};
use crate::filter;
use crate::routes;
use crate::state::AppState;
use crate::views;

struct Seen {
    announced: bool,
    health: Health,
}

/// Watch aria2 and announce transitions, independently of anyone looking.
///
/// The stream only runs while a page is open, and the whole point is not
/// having to keep one open. So this reads the poller's snapshots rather than
/// anything a view produces — and reads them rather than polling itself,
/// because a second poll of the same daemon on its own timer was exactly the
/// duplication that made going faster expensive.
///
/// It registers no interest in the rate. Watching the queue for something that
/// finished is not a thing that benefits from ten looks a second, and while
/// nobody has a page open the poller drops to `IDLE_POLL_MS` — which is this
/// task's old interval, arrived at from the other direction.
///
/// It seeds itself from the first poll rather than announcing what it finds:
/// on a restart everything already finished is still in the queue seeding, and
/// re-announcing all of it is the one behaviour that would make the messages
/// worth muting.
///
/// Health changes are written to the activity log from here rather than from
/// the stream, for the same reason the Telegram messages are: a transition
/// that happened while nobody had the page open is exactly the one worth
/// having a record of afterwards.
pub fn spawn_watcher(state: AppState) {
    tokio::spawn(async move {
        let mut snapshots = state.poll.subscribe();
        let mut seen: HashMap<String, Seen> = HashMap::new();
        let mut seeded = false;

        while snapshots.changed().await.is_ok() {
            let downloads = snapshots.borrow_and_update().downloads.clone();

            if !seeded {
                for d in downloads.iter() {
                    let terminal = d.status == Status::Error || d.is_finished();
                    // Announcing these again would be the one behaviour worth
                    // muting; sweeping them again is not. A download that
                    // finished while this process was not running never had a
                    // sweep at all, and on the ones that did it finds nothing
                    // left to delete.
                    if terminal && d.status != Status::Error {
                        sweep_garbage(&state, d).await;
                        state.preexisting.forget(&d.gid);
                    }
                    seen.insert(
                        d.gid.clone(),
                        Seen {
                            announced: terminal,
                            health: d.health(),
                        },
                    );
                }
                seeded = true;
                continue;
            }

            for d in downloads.iter() {
                let previous = seen.get(&d.gid);
                let already = previous.is_some_and(|s| s.announced);
                let health = d.health();

                // Only a change, and only for a download already being
                // watched: a first sighting is not a transition, and writing
                // one would put a spurious line at the head of every log.
                if let Some(previous) = previous
                    && previous.health != health
                {
                    state.activity.record(&d.gid, views::state_word(health));
                }

                let terminal = d.status == Status::Error || d.is_finished();
                if terminal && !already {
                    if d.status == Status::Error {
                        let error = d.error_message.as_deref().unwrap_or("unknown error");
                        state.activity.record(&d.gid, format!("failed — {error}"));
                        failed(&state, d.display_name(), error).await;
                    } else {
                        sweep_garbage(&state, d).await;
                        state.preexisting.forget(&d.gid);
                        state
                            .activity
                            .record(&d.gid, format!("finished — {}", d.dir));
                        finished(&state, d.display_name(), &d.dir).await;
                    }
                }
                seen.insert(
                    d.gid.clone(),
                    Seen {
                        announced: terminal,
                        health,
                    },
                );
            }

            // `tellStopped` is a bounded window, so a download eventually
            // falls out of it — forget it too, rather than growing forever.
            let live: HashMap<&str, ()> = downloads.iter().map(|d| (d.gid.as_str(), ())).collect();
            seen.retain(|gid, _| live.contains_key(gid.as_str()));
        }
    });
}

/// Delete the garbage a finished download brought with it.
///
/// aria2 downloads whole pieces regardless of `select-file`, so a small
/// deselected file sharing a piece with a selected one still reaches the
/// disk; and a download adopted after a restart may have had no selection
/// applied at all. Both are in aria2's file list for the download, which is
/// what this judges: only paths the torrent itself listed are candidates, so
/// a release added into a folder that already held artwork or metadata of its
/// own leaves those alone. Symlinks are never removed, every path goes through
/// `resolve_existing` immediately before the delete, and only directories this
/// emptied, below the destination, go with it.
async fn sweep_garbage(state: &AppState, d: &Download) {
    let dir = Path::new(&d.dir);
    let existing = state.preexisting.known(&d.gid).unwrap_or_default();
    let mut deleted = Vec::new();
    for f in &d.files {
        let path = Path::new(&f.path);
        if existing.contains(path) {
            continue;
        }
        // A listed path aria2 holds no bytes for may hold something else by
        // now, such as artwork a media server put there.
        if !filter::is_garbage(&f.path)
            || f.completed_length == 0
            || !path.starts_with(dir)
            || path == dir
            || !routes::remove_contained(state, &f.path).await
        {
            continue;
        }
        let _ = tokio::fs::remove_file(format!("{}.aria2", f.path)).await;
        tracing::info!(gid = %d.gid, path = %f.path, "removed garbage from a finished download");
        // The only thing that deletes from the library without anyone asking,
        // so it says so where the download's history is read.
        state
            .activity
            .record(&d.gid, format!("deleted junk — {}", f.path));
        deleted.push(path.to_path_buf());
    }
    routes::remove_emptied_dirs(state, dir, &deleted).await;
}

/// Escape for Telegram HTML. `&` first to avoid double-escaping entities.
pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

async fn send(state: &AppState, text: &str) {
    let Some(telegram) = &state.config.telegram else {
        return;
    };
    let url = format!(
        "https://api.telegram.org/bot{}/sendMessage",
        telegram.bot_token
    );
    let body = serde_json::json!({
        "chat_id": telegram.chat_id,
        "text": text,
        "parse_mode": "HTML",
    });
    if let Err(e) = state.http.post(&url).json(&body).send().await {
        tracing::warn!(error = %e, "failed to send telegram notification");
    }
}

pub async fn finished(state: &AppState, name: &str, dir: &str) {
    let text = format!("<b>{}</b>\nfinished — {}", esc(name), esc(dir));
    send(state, &text).await;
}

pub async fn failed(state: &AppState, name: &str, error: &str) {
    let text = format!("<b>{}</b>\nfailed — {}", esc(name), esc(error));
    send(state, &text).await;
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::aria2::{Aria2, File};
    use crate::auth::session::Sessions;
    use crate::config::{Config, Root};

    fn file(index: u32, path: &str, selected: bool) -> File {
        File {
            index,
            path: path.to_string(),
            length: 100,
            completed_length: 100,
            selected,
        }
    }

    fn download(dir: &std::path::Path, files: Vec<File>) -> Download {
        Download {
            status: Status::Complete,
            total_length: 100,
            completed_length: 100,
            dir: dir.to_string_lossy().into_owned(),
            files,
            ..Download::fixture()
        }
    }

    fn state(root: std::path::PathBuf) -> AppState {
        AppState {
            config: Arc::new(Config {
                roots: vec![Root {
                    name: "media".to_string(),
                    path: root,
                }],
                ..Config::fixture()
            }),
            aria2: Aria2::new(String::new(), reqwest::Client::new()),
            sessions: Sessions::new(),
            activity: crate::activity::Activity::new(),
            poll: crate::poll::Poll::new(),
            clearing: crate::state::Clearing::default(),
            adding: Default::default(),
            preexisting: Default::default(),
            http: reqwest::Client::new(),
        }
    }

    /// A fresh root, named for its test so the suite's threads never share a
    /// tree.
    async fn temp_root(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("mariastew-sweep-{name}"));
        let _ = tokio::fs::remove_dir_all(&root).await;
        tokio::fs::create_dir_all(&root).await.unwrap();
        root
    }

    async fn touch(path: &std::path::Path) {
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(path, b"bytes").await.unwrap();
    }

    /// The bug this exists to fix: a torrent completes with garbage files
    /// aria2 wrote anyway (piece-boundary spillover, not a selection
    /// mistake), and until this ran they just sat there. A file
    /// `filter::is_garbage` does not flag must survive whether or not aria2
    /// selected it — this is not "delete whatever is unselected".
    #[tokio::test]
    async fn sweep_garbage_removes_the_spillover_a_selection_could_not_stop() {
        let root = temp_root("spillover").await;
        let release = root.join("Some.Release");
        let movie = release.join("movie.mkv");
        let cover = release.join("cover.jpg");
        let cover_control = release.join("cover.jpg.aria2");
        let notes = release.join("notes.txt");
        for f in [&movie, &cover, &cover_control, &notes] {
            touch(f).await;
        }

        let d = download(
            &root,
            vec![
                file(1, movie.to_str().unwrap(), true),
                file(2, cover.to_str().unwrap(), false),
                file(3, notes.to_str().unwrap(), false),
            ],
        );
        sweep_garbage(&state(root.clone()), &d).await;

        assert!(movie.exists(), "a wanted file must never be removed");
        assert!(notes.exists(), "a file that is not garbage must survive");
        assert!(
            !cover.exists(),
            "garbage that landed anyway must be removed"
        );
        assert!(
            !cover_control.exists(),
            "the .aria2 control file must go with it"
        );

        let _ = tokio::fs::remove_dir_all(&root).await;
    }

    /// A download adopted after a restart had no selection applied, so its
    /// garbage is still `selected`; what the torrent listed is what counts.
    /// A file it never listed is not its to judge, however it is named — a
    /// release added into an existing folder must leave that folder's own
    /// artwork alone.
    #[tokio::test]
    async fn sweep_garbage_judges_what_the_torrent_listed_and_nothing_else() {
        let root = temp_root("unselected-tree").await;
        let release = root.join("Some.Release");
        let movie = release.join("movie.mkv");
        let nfo = release.join("release.nfo");
        let ds_store = release.join(".DS_Store");
        for f in [&movie, &nfo, &ds_store] {
            touch(f).await;
        }

        // Every file selected, and `.DS_Store` never listed — the shape of a
        // download this process did not start.
        let d = download(
            &root,
            vec![
                file(1, movie.to_str().unwrap(), true),
                file(2, nfo.to_str().unwrap(), true),
            ],
        );
        sweep_garbage(&state(root.clone()), &d).await;

        assert!(movie.exists());
        assert!(!nfo.exists(), "selection is not what makes a file garbage");
        assert!(
            ds_store.exists(),
            "a file the torrent never listed is not the sweep's to delete"
        );

        let _ = tokio::fs::remove_dir_all(&root).await;
    }

    /// A `Sample/` holding nothing but the sample is the case the script's
    /// trailing `prune` covered. Directories the sweep did not empty are left
    /// alone — `Subs/` is not this filter's business.
    #[tokio::test]
    async fn sweep_garbage_removes_the_directories_it_emptied() {
        let root = temp_root("empty-dirs").await;
        let release = root.join("Some.Release");
        let movie = release.join("movie.mkv");
        let sample = release.join("Sample/sample.mkv");
        let subs = release.join("Subs/english.srt");
        let art = release.join("Subs/poster.png");
        for f in [&movie, &sample, &subs, &art] {
            touch(f).await;
        }

        let d = download(
            &root,
            vec![
                file(1, movie.to_str().unwrap(), true),
                file(2, sample.to_str().unwrap(), false),
                file(3, subs.to_str().unwrap(), true),
                file(4, art.to_str().unwrap(), false),
            ],
        );
        sweep_garbage(&state(root.clone()), &d).await;

        assert!(!sample.exists());
        assert!(
            !release.join("Sample").exists(),
            "a directory the sweep emptied goes with its contents"
        );
        assert!(!art.exists());
        assert!(subs.exists(), "a directory with something left in it stays");
        assert!(release.exists());

        let _ = tokio::fs::remove_dir_all(&root).await;
    }

    /// The scope is the download, not the library. `dir` is a root shared
    /// with everything else that ever landed there, so a sweep that walked it
    /// would put every neighbouring release behind this filter on every
    /// completion.
    #[tokio::test]
    async fn sweep_garbage_never_leaves_the_downloads_own_entries() {
        let root = temp_root("scope").await;
        let mine = root.join("Some.Release/movie.mkv");
        let theirs = root.join("Another.Release/cover.jpg");
        let loose = root.join("cover.jpg");
        for f in [&mine, &theirs, &loose] {
            touch(f).await;
        }

        let d = download(&root, vec![file(1, mine.to_str().unwrap(), true)]);
        sweep_garbage(&state(root.clone()), &d).await;

        assert!(mine.exists());
        assert!(theirs.exists(), "another download's tree is not in scope");
        assert!(loose.exists(), "nor is the root the download landed in");

        let _ = tokio::fs::remove_dir_all(&root).await;
    }

    /// Links are read, never followed. A link is not a file this download
    /// wrote, and resolving one would let a name inside the tree decide what
    /// gets unlinked outside it.
    /// A file that was in the library before the download started is not the
    /// download's to sweep, whatever it is called.
    #[tokio::test]
    async fn sweep_garbage_leaves_a_file_that_was_there_first() {
        let root = temp_root("preexisting").await;
        let poster = root.join("Show/poster.jpg");
        touch(&poster).await;
        let state = state(root.clone());
        let d = download(&root, vec![file(1, poster.to_str().unwrap(), true)]);
        state
            .preexisting
            .record(&d.gid, std::collections::HashSet::from([poster.clone()]));

        sweep_garbage(&state, &d).await;

        assert!(
            poster.exists(),
            "a file that predates the download was swept"
        );
        let _ = tokio::fs::remove_dir_all(&root).await;
    }

    /// A directory link inside a root still reaches somewhere else in the
    /// library, and the file there is not this download's.
    #[tokio::test]
    async fn sweep_garbage_does_not_delete_through_a_linked_directory() {
        let root = temp_root("dir-symlink").await;
        let elsewhere = root.join("Another.Release");
        let poster = elsewhere.join("poster.jpg");
        touch(&poster).await;
        std::os::unix::fs::symlink(&elsewhere, root.join("Some.Release")).unwrap();

        let through_link = root.join("Some.Release/poster.jpg");
        let d = download(&root, vec![file(1, through_link.to_str().unwrap(), false)]);
        sweep_garbage(&state(root.clone()), &d).await;

        assert!(
            poster.exists(),
            "a file reached through a linked directory was deleted"
        );
        let _ = tokio::fs::remove_dir_all(&root).await;
    }

    /// A path the torrent lists but aria2 never wrote may hold something else
    /// by now, such as artwork a media server put there.
    #[tokio::test]
    async fn sweep_garbage_leaves_a_listed_path_aria2_never_wrote() {
        let root = temp_root("unwritten").await;
        let poster = root.join("Some.Release/poster.jpg");
        touch(&poster).await;

        let d = download(
            &root,
            vec![File {
                completed_length: 0,
                ..file(1, poster.to_str().unwrap(), false)
            }],
        );
        sweep_garbage(&state(root.clone()), &d).await;

        assert!(poster.exists(), "a file aria2 never wrote to was deleted");
        let _ = tokio::fs::remove_dir_all(&root).await;
    }

    #[tokio::test]
    async fn sweep_garbage_does_not_follow_a_symlink_out_of_the_tree() {
        let root = temp_root("symlink").await;
        let release = root.join("Some.Release");
        let movie = release.join("movie.mkv");
        let target = root.join("keep.jpg");
        touch(&movie).await;
        touch(&target).await;
        let link = release.join("cover.jpg");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let d = download(
            &root,
            vec![
                file(1, movie.to_str().unwrap(), true),
                file(2, link.to_str().unwrap(), false),
            ],
        );
        sweep_garbage(&state(root.clone()), &d).await;

        assert!(target.exists(), "a symlink's target must never be unlinked");
        assert!(
            tokio::fs::symlink_metadata(&link).await.is_ok(),
            "nor the link itself"
        );

        let _ = tokio::fs::remove_dir_all(&root).await;
    }

    /// The containment boundary applies here exactly as it does to every
    /// other delete in the service: a path aria2 reports is still just a
    /// report, re-checked against the canonicalised roots before anything is
    /// unlinked. Garbage sitting outside every configured root — a download
    /// aria2 was pointed somewhere the roots no longer cover — must be left
    /// alone even though every other condition for deletion is met.
    #[tokio::test]
    async fn sweep_garbage_refuses_a_path_outside_every_root() {
        let root = temp_root("root-only").await;
        let elsewhere = temp_root("outside").await;
        let cover = elsewhere.join("Some.Release/cover.jpg");
        touch(&cover).await;

        let d = download(&elsewhere, vec![file(1, cover.to_str().unwrap(), true)]);
        sweep_garbage(&state(root.clone()), &d).await;

        assert!(
            cover.exists(),
            "a path outside every configured root must never be deleted"
        );

        let _ = tokio::fs::remove_dir_all(&root).await;
        let _ = tokio::fs::remove_dir_all(&elsewhere).await;
    }

    #[test]
    fn esc_escapes_all_three_markup_characters() {
        assert_eq!(
            esc("<b>Tom & Jerry</b>"),
            "&lt;b&gt;Tom &amp; Jerry&lt;/b&gt;"
        );
    }

    /// Escaping `&` first means the entities this just produced for `<` and
    /// `>` never get run back through the `&` replacement.
    #[test]
    fn esc_does_not_double_escape_the_entities_it_just_wrote() {
        assert_eq!(esc("<"), "&lt;");
        assert_eq!(esc(">"), "&gt;");
    }
}
