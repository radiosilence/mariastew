//! The live streams, one per page with live parts, named as the page is. There
//! is one: the downloads ([`downloads`]).
//!
//! A stream reads the process-wide poller's snapshots (`poll`) rather than
//! asking aria2 itself, so a second open page does not double what aria2 is
//! asked, and it starts no work of its own: the poller samples faster while
//! any page holds a [`crate::poll::Viewer`], and that is all opening a stream
//! does. The snapshots are paced by `askama_datastar::paced`, at most four
//! redraws a second, and the stream is ended after five minutes for the page's
//! `reopen_on` to open another, which bounds how long a page holds the poller
//! at its fast rate over a connection that died without either end noticing (a
//! suspended phone, a replaced pod). The page opens a stream afresh when its
//! address changes, so a stream never follows navigation.

use std::time::Duration;

use askama_datastar::{Pace, Signals, paced};
use axum::extract::State;
use axum::response::Response;
use futures_util::StreamExt as _;

use crate::auth::extract::CurrentSession;
use crate::state::AppState;

pub(super) mod downloads;

/// At most four redraws a second, and a five-minute life. The life is longer
/// than slsk's minute because a stream that ends leaves the bars still for up
/// to five seconds before `reopen_on` notices, and a download page is watched
/// for its bars.
const PACE: Pace = Pace {
    throttle: Duration::from_millis(250),
    lifetime: Duration::from_secs(300),
};

/// How often a stream checks that its session still exists. A session signed
/// out elsewhere is never checked again by anything else while the stream is
/// open, which would leave a live feed of the queue attached to a browser no
/// longer permitted to see it.
const SESSION_RECHECK: Duration = Duration::from_secs(1);

/// What the page shows, from the signals Datastar sends with the stream's
/// request.
#[derive(serde::Deserialize, Default)]
#[serde(default)]
pub(super) struct Shown {
    /// What the page has drawn of the list: as its body drew it, then as each
    /// stream before this one left it. None from a page that does not say,
    /// which is drawn whole.
    seen: Option<downloads::Seen>,
}

/// The downloads page's stream.
pub(super) async fn serve(
    State(state): State<AppState>,
    CurrentSession(session): CurrentSession,
    Signals(shown): Signals<Shown>,
) -> Response {
    // Held inside the stream: dropping it is what tells the poller this page
    // is gone, and it goes when the response does. It ends when the process
    // shuts down, which ends the stream; the page reopens it on the new pod.
    let viewer = state.poll.viewer();
    let snapshots = paced(
        futures_util::stream::unfold(viewer, |mut viewer| async move {
            viewer.next().await.map(|snapshot| (snapshot, viewer))
        }),
        PACE,
    );
    let events = async_stream::stream! {
        let mut list = downloads::Downloads::open(shown.seen);
        let mut snapshots = std::pin::pin!(snapshots);
        let mut checked = std::time::Instant::now() - SESSION_RECHECK;
        while let Some(snapshot) = snapshots.next().await {
            if checked.elapsed() >= SESSION_RECHECK {
                checked = std::time::Instant::now();
                if state.sessions.get(&session.id).await.is_none() {
                    break;
                }
            }
            for event in list.draw(&state, &snapshot) {
                yield event;
            }
        }
    };
    askama_datastar::stream(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stream ends when the server shuts down, so graceful shutdown is not
    /// held open by it; the page's `reopen_on` opens one on the next pod.
    #[tokio::test]
    async fn shutting_down_ends_the_stream() {
        let state = AppState::fixture(String::new());
        state.poll.close();
        let session = crate::auth::session::Session {
            id: "s".to_string(),
            sub: "tester".to_string(),
            expires: std::time::Instant::now() + Duration::from_secs(60),
        };
        let response = serve(
            State(state),
            CurrentSession(session),
            Signals(Shown::default()),
        )
        .await;
        let body = tokio::time::timeout(
            Duration::from_secs(1),
            axum::body::to_bytes(response.into_body(), usize::MAX),
        )
        .await
        .expect("the stream ends once closed");
        assert!(body.is_ok(), "the stream errored rather than ending");
    }
}
