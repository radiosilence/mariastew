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
//! first pass sends only what changed after the body was drawn, and a stream
//! ending sets `seen` to what it drew, so the next one carries on from there.

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

/// A short hash (FNV-1a) of `markup` as a stream compares it ([`unnamed`]),
/// stable across builds, so a page and the stream after it agree on it.
pub(in crate::web) fn fingerprint(markup: &str) -> String {
    let hash = unnamed(markup)
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        });
    format!("{hash:016x}")
}

/// Markup without the busy signals' names. The kit names each request
/// button's own busy signal afresh on every render, so a row drawn twice from
/// the same state differs only in those; left in, every pass would send every
/// row. A row sent for another change brings its new names with it.
fn unnamed(html: &str) -> String {
    const SIGNAL: &str = "_busy";
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(at) = rest.find(SIGNAL) {
        let (before, after) = rest.split_at(at + SIGNAL.len());
        out.push_str(before);
        let hex = after
            .bytes()
            .take(12)
            .take_while(u8::is_ascii_hexdigit)
            .count();
        rest = if hex == 12 { &after[12..] } else { after };
    }
    out.push_str(rest);
    out
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

    /// What `snapshot` changes on the page.
    pub(in crate::web) fn draw(&mut self, state: &AppState, snapshot: &Snapshot) -> Vec<Event> {
        let Some(downloads) = &snapshot.downloads else {
            // Sent once per outage; the first answer after it sends the list.
            if self.seen.as_ref().is_some_and(|seen| seen.down) {
                return Vec::new();
            }
            self.seen = Some(Seen::unreachable());
            return vec![List::unreachable().draw().patch().into()];
        };
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

    /// `seen` as this stream leaves the page.
    pub(in crate::web) fn seen(&self) -> Option<Event> {
        let seen = self.seen.as_ref()?;
        let json = serde_json::json!({ "seen": seen }).to_string();
        Some(PatchSignals::new(json).into())
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
        assert_eq!(list.draw(&state, &down).len(), 1);
        assert!(list.draw(&state, &down).is_empty());
        let back = Snapshot {
            downloads: Some(std::sync::Arc::new(Vec::new())),
        };
        assert_eq!(list.draw(&state, &back).len(), 1);
        assert!(list.draw(&state, &back).is_empty());
    }

    #[test]
    fn a_fingerprint_ignores_the_busy_signals_names_and_nothing_else() {
        assert_eq!(
            fingerprint(r#"<b data-indicator="_busy0123456789ab">x</b>"#),
            fingerprint(r#"<b data-indicator="_busyba9876543210">x</b>"#)
        );
        assert_ne!(
            fingerprint(r#"<b data-indicator="_busy0123456789ab">x</b>"#),
            fingerprint(r#"<b data-indicator="_busy0123456789ab">y</b>"#)
        );
    }

    #[test]
    fn a_fingerprint_follows_the_markup() {
        assert_eq!(fingerprint("<p>a</p>"), fingerprint("<p>a</p>"));
        assert_ne!(fingerprint("<p>a</p>"), fingerprint("<p>b</p>"));
    }
}
