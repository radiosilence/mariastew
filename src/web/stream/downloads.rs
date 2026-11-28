//! The downloads page live: each download as it changes, and the whole list
//! only when one arrives, leaves or moves.
//!
//! A row is compared by a fingerprint of its markup against what went down
//! this connection, not against the previous snapshot: a client too slow to
//! collect every pass skips some, and a row that changed in one it missed
//! still has to reach it. A seeding torrent is sent once. Datastar morphs by
//! id, which can neither add an element nor remove one, so a change in which
//! downloads there are, or their order, sends `#downloads` whole.
//!
//! The page's body says what it drew in the `seen` signal, so a stream's
//! first pass sends only what changed after the body was drawn. Every pass
//! that sends a region sets `seen` in the same answer, so the page's `seen`
//! always matches what it shows, however the stream ends: Datastar drops a
//! stream when the page is hidden or the network fails, and the one that
//! replaces it starts from what the page really has.

use askama_datastar::PatchSignals;
use axum::response::sse::Event;

use crate::poll::Snapshot;
use crate::state::AppState;
use crate::web::Draw;
use crate::web::page::downloads::{Drawn, List, drawn};

/// What the page shows of the list.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::web) struct Seen {
    /// Each download drawn, in order: its gid and its markup's fingerprint.
    rows: Vec<(String, String)>,
    /// The list is the warning that aria2 is unreachable.
    down: bool,
}

impl Seen {
    pub(in crate::web) fn of(drawn: &[Drawn]) -> Self {
        Self {
            rows: drawn
                .iter()
                .map(|d| (d.gid.clone(), d.hash.clone()))
                .collect(),
            down: false,
        }
    }

    pub(in crate::web) fn unreachable() -> Self {
        Self {
            rows: Vec::new(),
            down: true,
        }
    }
}

/// A short hash (FNV-1a) of `markup`, stable across builds, so a page and the
/// stream after it agree on it. The kit names a control's busy signal by what
/// the control is, so a row drawn twice from the same state is the same
/// markup byte for byte.
pub(in crate::web) fn fingerprint(markup: &str) -> String {
    let hash = markup.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{hash:016x}")
}

/// Which of `now`'s rows the page does not have, or `None` when the list
/// itself changed and is sent whole.
fn unsent(page: Option<&Seen>, now: &Seen) -> Option<Vec<usize>> {
    let page = page.filter(|page| !page.down && !now.down)?;
    let same_downloads = page.rows.len() == now.rows.len()
        && page
            .rows
            .iter()
            .zip(&now.rows)
            .all(|((a, _), (b, _))| a == b);
    same_downloads.then(|| {
        page.rows
            .iter()
            .zip(&now.rows)
            .enumerate()
            .filter(|(_, ((_, a), (_, b)))| a != b)
            .map(|(i, _)| i)
            .collect()
    })
}

/// The list, as one stream has sent it.
pub(in crate::web) struct Downloads {
    seen: Option<Seen>,
}

impl Downloads {
    pub(in crate::web) fn open(seen: Option<Seen>) -> Self {
        Self { seen }
    }

    /// What `snapshot` changes on the page, and `seen` as the page will have
    /// it, whenever anything changed.
    pub(in crate::web) fn draw(&mut self, state: &AppState, snapshot: &Snapshot) -> Vec<Event> {
        let mut events: Vec<Event> = match &snapshot.downloads {
            // Sent once per outage; the first answer after it sends the list.
            None if self.seen.as_ref().is_some_and(|seen| seen.down) => Vec::new(),
            None => {
                self.seen = Some(Seen::unreachable());
                vec![List::unreachable().draw().patch().into()]
            }
            Some(downloads) => {
                let drawn = drawn(state, downloads);
                let now = Seen::of(&drawn);
                let events = match unsent(self.seen.as_ref(), &now) {
                    Some(rows) => rows
                        .into_iter()
                        .map(|i| drawn[i].html.patch().into())
                        .collect(),
                    None => vec![List::of(&drawn).draw().patch().into()],
                };
                self.seen = Some(now);
                events
            }
        };
        if !events.is_empty()
            && let Some(seen) = &self.seen
        {
            let json = serde_json::json!({ "seen": seen }).to_string();
            events.push(PatchSignals::new(json).into());
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(rows: &[(&str, &str)]) -> Seen {
        Seen {
            rows: rows
                .iter()
                .map(|(g, h)| (g.to_string(), h.to_string()))
                .collect(),
            down: false,
        }
    }

    /// The saving: a seeding torrent's markup is the same every pass.
    #[test]
    fn a_row_that_did_not_change_is_not_sent_again() {
        let page = seen(&[("a", "1"), ("b", "2")]);
        assert_eq!(unsent(Some(&page), &page), Some(vec![]));
        assert_eq!(
            unsent(Some(&page), &seen(&[("a", "1"), ("b", "9")])),
            Some(vec![1])
        );
    }

    /// Morphing by id can neither add an element nor remove one, and leaves
    /// moved ones where they were.
    #[test]
    fn a_download_arriving_leaving_or_moving_sends_the_list() {
        let page = seen(&[("a", "1"), ("b", "2")]);
        assert_eq!(
            unsent(Some(&page), &seen(&[("a", "1"), ("b", "2"), ("c", "3")])),
            None
        );
        assert_eq!(unsent(Some(&page), &seen(&[("a", "1")])), None);
        assert_eq!(unsent(Some(&page), &seen(&[("b", "2"), ("a", "1")])), None);
    }

    /// A page that does not say what it drew, or one showing the outage
    /// warning, is sent the list.
    #[test]
    fn a_page_that_has_no_list_is_sent_one() {
        assert_eq!(unsent(None, &seen(&[])), None);
        assert_eq!(unsent(Some(&Seen::unreachable()), &seen(&[])), None);
        // An empty list the page already shows needs nothing.
        assert_eq!(unsent(Some(&seen(&[])), &seen(&[])), Some(vec![]));
    }

    /// An outage is said once, and the first answer after it sends the list,
    /// which is what retires the warning.
    #[test]
    fn an_outage_is_said_once_and_retired_by_the_next_list() {
        let state = AppState::fixture(String::new());
        let mut list = Downloads::open(Some(seen(&[])));
        let down = Snapshot { downloads: None };
        // The warning, and `seen` with it.
        assert_eq!(list.draw(&state, &down).len(), 2);
        assert!(list.draw(&state, &down).is_empty());
        let back = Snapshot {
            downloads: Some(std::sync::Arc::new(Vec::new())),
        };
        assert_eq!(list.draw(&state, &back).len(), 2);
        assert!(list.draw(&state, &back).is_empty());
    }

    fn snapshot(gids: &[&str]) -> Snapshot {
        Snapshot {
            downloads: Some(std::sync::Arc::new(
                gids.iter()
                    .map(|gid| crate::aria2::Download {
                        gid: gid.to_string(),
                        ..crate::aria2::Download::fixture()
                    })
                    .collect(),
            )),
        }
    }

    /// A stream dropped without ending (a hidden page, a lost network) sends
    /// nothing as it goes, so the page's `seen` must already match what it
    /// shows. The page loads with a and b, c arrives and is drawn, the stream
    /// is dropped, and c leaves: the stream opened with the page's `seen` has
    /// to take c off the page.
    #[test]
    fn a_stream_dropped_mid_way_leaves_the_page_a_seen_that_matches_it() {
        let state = AppState::fixture(String::new());
        let loaded = snapshot(&["a", "b"]);
        let loaded = Seen::of(&drawn(&state, loaded.downloads.as_deref().unwrap()));
        let mut first = Downloads::open(Some(loaded.clone()));
        let sent = first.draw(&state, &snapshot(&["a", "b", "c"]));
        assert_eq!(sent.len(), 2, "the list, and `seen` with it");
        // What the page holds once that answer landed.
        let page = first.seen.clone();
        assert_ne!(page.as_ref(), Some(&loaded));

        let mut next = Downloads::open(page);
        assert!(
            !next.draw(&state, &snapshot(&["a", "b"])).is_empty(),
            "c stayed on the page"
        );
    }

    /// The saving rests on this: a row with request buttons, drawn twice from
    /// one state, is the same markup, busy signals and all.
    #[test]
    fn a_download_drawn_twice_is_the_same_markup() {
        let state = AppState::fixture(String::new());
        let snapshot = snapshot(&["a", "b"]);
        let downloads = snapshot.downloads.as_deref().unwrap();
        let (first, second) = (drawn(&state, downloads), drawn(&state, downloads));
        assert!(first[0].html.as_str().contains("_busy"));
        for (a, b) in first.iter().zip(&second) {
            assert_eq!(a.html.as_str(), b.html.as_str());
        }
    }

    #[test]
    fn a_fingerprint_follows_the_markup() {
        assert_eq!(fingerprint("<p>a</p>"), fingerprint("<p>a</p>"));
        assert_ne!(fingerprint("<p>a</p>"), fingerprint("<p>b</p>"));
    }
}
