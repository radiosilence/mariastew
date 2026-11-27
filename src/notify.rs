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

use askama::Template;

use crate::aria2::{Health, Status};
use crate::library;
use crate::state::AppState;

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
            let Some(downloads) = snapshots.borrow_and_update().downloads.clone() else {
                continue;
            };

            if !seeded {
                for d in downloads.iter() {
                    let terminal = d.status == Status::Error || d.is_finished();
                    // Announcing these again would be the one behaviour worth
                    // muting; sweeping them again is not. A download that
                    // finished while this process was not running never had a
                    // sweep at all, and on the ones that did it finds nothing
                    // left to delete.
                    if terminal && d.status != Status::Error {
                        library::sweep(&state, d).await;
                        state.preexisting.forget(&d.gid).await;
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
                    state.activity.record(&d.gid, health.word());
                }

                let terminal = d.status == Status::Error || d.is_finished();
                if terminal && !already {
                    if d.status == Status::Error {
                        let error = d.error_message.as_deref().unwrap_or("unknown error");
                        state.activity.record(&d.gid, format!("failed — {error}"));
                        failed(&state, d.display_name(), error).await;
                    } else {
                        library::sweep(&state, d).await;
                        state.preexisting.forget(&d.gid).await;
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
    // `without_url`: the URL carries the bot token.
    let sent = state.http.post(&url).json(&body).send().await;
    if let Err(e) = sent.and_then(reqwest::Response::error_for_status) {
        tracing::warn!(error = %e.without_url(), "failed to send telegram notification");
    }
}

/// A download that finished, in Telegram's HTML. The `.html` extension is what
/// makes askama escape the name and the path, which Telegram would otherwise
/// read as markup.
#[derive(Template)]
#[template(path = "notify/finished.html")]
struct Finished<'a> {
    name: &'a str,
    dir: &'a str,
}

/// A download that failed, in Telegram's HTML.
#[derive(Template)]
#[template(path = "notify/failed.html")]
struct Failed<'a> {
    name: &'a str,
    error: &'a str,
}

/// Sends `message`, or logs why it could not be drawn.
async fn announce(state: &AppState, message: impl Template) {
    match message.render() {
        Ok(text) => send(state, &text).await,
        Err(e) => tracing::warn!(error = %e, "could not draw a telegram notification"),
    }
}

pub async fn finished(state: &AppState, name: &str, dir: &str) {
    announce(state, Finished { name, dir }).await;
}

pub async fn failed(state: &AppState, name: &str, error: &str) {
    announce(state, Failed { name, error }).await;
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::aria2::{Download, File};
    use crate::config::{Config, Root};

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
            ..AppState::fixture(String::new())
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

    /// An outage is not every download leaving the list. Treated as one, the
    /// first answer after it re-announces everything already finished.
    #[tokio::test]
    async fn an_aria2_outage_announces_nothing_when_it_ends() {
        let root = temp_root("outage").await;
        let state = state(root.clone());
        let finished = Download {
            gid: "done".to_string(),
            ..download(&root, Vec::new())
        };
        let fresh = Download {
            gid: "fresh".to_string(),
            ..download(&root, Vec::new())
        };
        let reachable = |downloads: Vec<Download>| crate::poll::Snapshot {
            downloads: Some(Arc::new(downloads)),
        };
        spawn_watcher(state.clone());
        tokio::task::yield_now().await;

        for snapshot in [
            reachable(vec![finished.clone()]),
            crate::poll::Snapshot { downloads: None },
            reachable(vec![finished.clone(), fresh]),
        ] {
            state.poll.publish(snapshot);
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        assert!(
            !state.activity.entries("fresh").is_empty(),
            "the watcher never saw the snapshot after the outage"
        );
        assert!(
            state.activity.entries("done").is_empty(),
            "a download finished before the outage was announced again after it"
        );
        let _ = tokio::fs::remove_dir_all(&root).await;
    }

    /// Telegram reads the message as HTML, so a name or a reason with markup
    /// in it is escaped exactly once, and the message's own markup is not.
    #[test]
    fn a_notification_escapes_the_download_and_keeps_its_own_markup() {
        use askama_datastar::escaping::{Guard, escaped};
        const HOSTILE: &str = r#"<em>Tom & "Jerry"</em>"#;
        let guard = Guard::new(&[HOSTILE])
            .templates(concat!(env!("CARGO_MANIFEST_DIR"), "/templates/notify"))
            .unwrap();
        for text in [
            Finished {
                name: HOSTILE,
                dir: HOSTILE,
            }
            .render()
            .unwrap(),
            Failed {
                name: HOSTILE,
                error: HOSTILE,
            }
            .render()
            .unwrap(),
        ] {
            assert_eq!(guard.problems(&text), [], "{text}");
            assert!(text.starts_with("<b>"), "{text}");
            assert!(text.contains(&escaped(HOSTILE)), "{text}");
        }
    }
}
