//! The downloads page: the header's add button and its sheet ([`add`], with
//! the [`destination`] picker), and the list, one disclosure per download, kept
//! live by the page's stream (`stream::downloads`).
//!
//! aria2's shapes do not reach a template. Everything a download shows is
//! worked out once in [`Download::new`] (the numbers formatted, the health
//! resolved to a tone and a word), so the markup makes no decisions.
//!
//! The bar, the bytes, the rate and the time left are one `kit::Transfer`,
//! which the page moves between the stream's readings, so the row advances
//! smoothly rather than in steps as each reading lands.

use std::time::Duration;

use askama::Template;
use askama_datastar::Html;
use axum::extract::{Path, State};
use axum::response::Response;
use koan_kit_web as kit;

use crate::activity::Entry;
use crate::aria2::{self, Health, Status};
use crate::auth::extract::CurrentSession;
use crate::library::Removed;
use crate::state::AppState;
use crate::web::stream::downloads::{Seen, fingerprint};
use crate::web::{Draw, failed};

pub(in crate::web) mod add;
pub(in crate::web) mod destination;

/// The page's body.
#[derive(Template)]
#[template(path = "downloads/page.html")]
pub(in crate::web) struct Page {
    list: Html,
}

impl Page {
    /// The page as aria2 has it now, and what of it the stream need not send
    /// again.
    ///
    /// A restarted aria2 sidecar does not blank the page: the add sheet is
    /// still useful without a list, and the stream fills the list in the moment
    /// aria2 answers. Only the actions still fail, since a silent success there
    /// would be the wrong kind of tolerant.
    pub(in crate::web) async fn of(state: &AppState) -> (Self, Seen) {
        let (list, seen) = match state.aria2.downloads().await {
            Ok(downloads) => {
                let drawn = drawn(state, &downloads);
                (List::of(&drawn).draw(), Seen::of(&drawn))
            }
            Err(e) => {
                tracing::warn!(error = %e, "page: aria2 unreachable, drawing the warning");
                (List::unreachable().draw(), Seen::unreachable())
            }
        };
        (Page { list }, seen)
    }
}

/// The list, `#downloads`: every download, or what stands in for none. Sent
/// whole only when a download arrives, leaves or moves, since morphing by id
/// can neither add an element nor remove one.
#[derive(Template)]
#[template(path = "downloads/list.html")]
pub(in crate::web) struct List {
    downloads: Vec<Html>,
    /// aria2 did not answer: the list is the warning, until it does.
    unreachable: bool,
}

impl List {
    pub(in crate::web) fn of(drawn: &[Drawn]) -> Self {
        Self {
            downloads: drawn.iter().map(|d| d.html.clone()).collect(),
            unreachable: false,
        }
    }

    pub(in crate::web) fn unreachable() -> Self {
        Self {
            downloads: Vec::new(),
            unreachable: true,
        }
    }
}

/// A download as drawn, with what a stream compares it by.
pub(in crate::web) struct Drawn {
    pub(in crate::web) gid: String,
    pub(in crate::web) html: Html,
    pub(in crate::web) hash: String,
}

/// Every download drawn, each with its own history. The log is read per
/// download because it is keyed by gid, and one with none (anything in aria2
/// before this pod started) is drawn without the panel.
pub(in crate::web) fn drawn(state: &AppState, downloads: &[aria2::Download]) -> Vec<Drawn> {
    downloads
        .iter()
        .map(|d| {
            let html = Download::new(
                d,
                state.activity.entries(&d.gid),
                state.clearing.contains(&d.gid),
                state.config.poll_interval,
            )
            .draw();
            Drawn {
                gid: d.gid.clone(),
                hash: fingerprint(html.as_str()),
                html,
            }
        })
        .collect()
}

/// Files listed before the rest are folded into a count. A season pack is
/// twenty entries and reads fine; a discography is five hundred and would be
/// the largest thing on the page, sent again whenever its download changes.
const MAX_FILES_SHOWN: usize = 24;

/// One download: `#dl-<gid>`, a disclosure whose summary says how it is going
/// and whose body says what aria2 is doing.
#[derive(Template)]
#[template(path = "downloads/download.html")]
pub(in crate::web) struct Download {
    gid: String,
    name: String,
    /// The bar's numbers (`aria2::Download::progress_totals`): a total of
    /// nought is one not yet known, which the meter draws as waiting.
    bytes: u64,
    total: u64,
    /// The rate the bar's bytes move at (`progress_rate`), or aria2's own
    /// while a magnet has no size. Nought whenever nothing is being
    /// downloaded: a hash check, a finished or paused row. The page moves the
    /// bytes only at a rate above nought.
    rate: u64,
    /// When the reading last changed (`aria2::stamp`).
    measured_at_ms: i64,
    /// How far apart readings arrive: the poll's sleep and the round trip
    /// that brought this one. The page moves a reading for two of these, so a
    /// row runs on until the next is due and stops if it does not come.
    every: Duration,
    /// The meter's and the status's tone: the fast way to read a row at arm's
    /// length. The word beside it says the same for anyone who cannot see it.
    tone: kit::Tone,
    state: &'static str,
    /// Absent when idle: 0 B/s reads as a measured standstill. `down` is
    /// aria2's own rate, the whole torrent's where the transfer's is scaled
    /// to the chosen files.
    down: Option<String>,
    up: Option<String>,
    uploaded: String,
    ratio: Option<String>,
    connections: u32,
    seeders: u32,
    /// How aria2 is cutting the torrent up, which explains both lumpy progress
    /// and a deselected file landing anyway.
    pieces: Option<String>,
    info_hash: Option<String>,
    error: Option<String>,
    /// aria2's error code in words, which is the reason as often as not, since
    /// aria2 often leaves the message empty.
    error_code: Option<String>,
    dir: String,
    files: Vec<File>,
    file_count: usize,
    selected_count: usize,
    /// "N skipped, X bytes", or none.
    skipped: Option<String>,
    log: Vec<Line>,
    finished: bool,
    paused: bool,
    /// Removal asked for and not yet confirmed (`state::Clearing`). It
    /// replaces the state rather than sitting beside it: a row on its way out
    /// is not "downloaded", whatever aria2 still says about the swarm.
    clearing: bool,
}

/// A file of the torrent. A deselected file is listed, not hidden: what the
/// filter refused was otherwise visible only in the log.
struct File {
    name: String,
    size: String,
    /// Selected files only: a deselected file's own bar would report only
    /// piece-boundary spillover, which is not progress towards anything.
    percent: Option<f64>,
    selected: bool,
}

/// A line of the download's history, at epoch milliseconds, which the kit's
/// `Time` shows in the reader's own zone.
struct Line {
    at_ms: i64,
    message: String,
}

/// A health's tone. Colour is never the only reading: the word
/// (`Health::word`) goes beside it.
fn tone(health: Health) -> kit::Tone {
    match health {
        Health::Moving | Health::Complete => kit::Tone::Ok,
        Health::Choked => kit::Tone::Warn,
        Health::Blocked | Health::Errored | Health::Dead => kit::Tone::Bad,
        Health::Queued | Health::Resolving | Health::Verifying | Health::Paused => kit::Tone::Wait,
    }
}

/// The last path segment, the only part of a file worth a row of its own:
/// every file in a torrent shares the rest with its siblings.
fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

impl Download {
    pub(in crate::web) fn new(
        d: &aria2::Download,
        log: Vec<Entry>,
        clearing: bool,
        poll: Duration,
    ) -> Self {
        let health = d.health();
        // Not a health: nothing has been asked of the swarm, so this is a fact
        // about a request in flight. It still outranks the reading:
        // "downloaded" beside a spinner is the confusion the mark removes.
        let (tone, state) = if clearing {
            (kit::Tone::Wait, "clearing")
        } else {
            (tone(health), health.word())
        };
        // The bar's own numbers, so "230 MB of 754 MB" beside a bar at 30% is
        // the hash check's progress and not two readings of different things.
        let (total, bytes) = d.progress_totals();
        let fetching = d.verified_length.is_none() && !d.is_finished();
        let (skipped_count, skipped_bytes) = d
            .files
            .iter()
            .filter(|f| !f.selected)
            .fold((0usize, 0u64), |(n, b), f| (n + 1, b + f.length));

        Download {
            gid: d.gid.clone(),
            name: d.display_name().to_string(),
            bytes,
            total,
            // A magnet without a size has no bytes to move, so its transfer
            // is a waiting meter and aria2's rate while there is one.
            rate: match (fetching, total) {
                (false, _) => 0,
                (true, 0) => d.download_speed,
                (true, _) => d.progress_rate(),
            },
            measured_at_ms: d.measured_at_ms,
            every: poll + d.read_in,
            tone,
            state,
            down: (d.download_speed > 0).then(|| kit::rate(d.download_speed)),
            up: (d.upload_speed > 0).then(|| kit::rate(d.upload_speed)),
            uploaded: kit::size(d.upload_length),
            ratio: d.ratio().map(|r| format!("{r:.2}")),
            connections: d.connections,
            seeders: d.num_seeders,
            pieces: (d.num_pieces > 0)
                .then(|| format!("{} × {}", d.num_pieces, kit::size(d.piece_length))),
            info_hash: d.info_hash.clone(),
            error: d.error_message.clone().filter(|m| !m.is_empty()),
            error_code: d
                .error_code
                .map(|code| match aria2::error_code_meaning(code) {
                    Some(meaning) => format!("aria2 code {code} — {meaning}"),
                    None => format!("aria2 code {code}"),
                }),
            dir: d.dir.clone(),
            files: d
                .files
                .iter()
                .take(MAX_FILES_SHOWN)
                .map(|f| File {
                    name: basename(&f.path).to_string(),
                    size: kit::size(f.length),
                    percent: (f.selected && f.length > 0)
                        .then(|| (f.completed_length as f64 / f.length as f64 * 100.0).round()),
                    selected: f.selected,
                })
                .collect(),
            file_count: d.files.len(),
            selected_count: d.files.len() - skipped_count,
            skipped: (skipped_count > 0)
                .then(|| format!("{skipped_count} skipped, {}", kit::size(skipped_bytes))),
            log: log
                .into_iter()
                .map(|e| Line {
                    at_ms: e
                        .at
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as i64,
                    message: e.message,
                })
                .collect(),
            finished: d.is_finished(),
            paused: d.status == Status::Paused,
            clearing,
        }
    }

    /// The disclosure's id, the region a stream sends again.
    fn id(&self) -> String {
        format!("dl-{}", self.gid)
    }

    /// The files panel's id, which keeps it open across a morph.
    fn files_id(&self) -> String {
        format!("files-{}", self.gid)
    }

    /// The log panel's id, which keeps it open across a morph.
    fn log_id(&self) -> String {
        format!("log-{}", self.gid)
    }

    /// The address of an action on this download.
    fn url(&self, verb: &str) -> String {
        format!("/downloads/{}/{verb}", self.gid)
    }

    fn status(&self) -> kit::Status<'_> {
        kit::Status::new(self.tone, self.state)
            .busy(self.clearing)
            .busy_with(&self.gid)
            .class("shrink-0")
    }

    fn transfer(&self) -> kit::Transfer<'static> {
        kit::Transfer::new(self.bytes, self.total)
            .rate(self.rate, self.measured_at_ms)
            .every(self.every)
            .tone(self.tone)
            .label("download")
            .class("mt-2")
    }

    fn file_meter(&self, percent: impl Into<kit::Amount>) -> kit::Meter<'static> {
        kit::Meter::new(percent, 100.0)
            .tone(self.tone)
            .label("file")
            .class("mt-1")
    }

    /// The readings the transfer line does not give: the whole torrent's
    /// rate, whether it is giving anything back, who it is talking to, how
    /// aria2 has cut it up.
    fn readings(&self) -> kit::Pairs<'_> {
        let dash = |v: &Option<String>| v.clone().unwrap_or_else(|| "—".to_string());
        kit::Pairs::new()
            .pair("down", dash(&self.down))
            .pair("up", dash(&self.up))
            .pair(
                "peers",
                format!("{} of {} seeding", self.connections, self.seeders),
            )
            // Total uploaded rather than the rate: the question a seeding row
            // raises is whether it has given anything back.
            .pair(
                "shared",
                match &self.ratio {
                    Some(ratio) => format!("{} · {ratio}×", self.uploaded),
                    None => self.uploaded.clone(),
                },
            )
            .optional("pieces", self.pieces.as_deref())
            .optional("hash", self.info_hash.as_deref())
            .class("text-fine")
    }
}

/// Pause a download. The stream draws the change, so the answer is empty.
pub(in crate::web) async fn pause(
    State(state): State<AppState>,
    Path(gid): Path<String>,
) -> Response {
    match state.aria2.pause(&gid).await {
        Ok(()) => {
            state.activity.record(&gid, "paused");
            askama_datastar::events([])
        }
        Err(e) => {
            tracing::warn!(%gid, error = %e, "pause refused");
            failed("aria2 would not pause it.")
        }
    }
}

/// Resume a download. The stream draws the change, so the answer is empty.
pub(in crate::web) async fn resume(
    State(state): State<AppState>,
    Path(gid): Path<String>,
) -> Response {
    match state.aria2.unpause(&gid).await {
        Ok(()) => {
            state.activity.record(&gid, "resumed");
            askama_datastar::events([])
        }
        Err(e) => {
            tracing::warn!(%gid, error = %e, "resume refused");
            failed("aria2 would not resume it.")
        }
    }
}

/// Clear a download: a finished one leaves the list with its files, an
/// unfinished one goes with what it wrote (`library::remove`). The answer
/// waits for aria2 to let go, so the button's own wait is the wait. The stream
/// draws the row going; the answer closes the question, if one was asked, or
/// says why it is still there.
pub(in crate::web) async fn remove(
    State(state): State<AppState>,
    CurrentSession(session): CurrentSession,
    Path(gid): Path<String>,
) -> Response {
    match crate::library::remove(&state, &session.sub, &gid).await {
        Ok(Removed::Gone | Removed::AlreadyClearing) => {
            askama_datastar::events([kit::Confirm::close().into()])
        }
        Ok(Removed::Held) => askama_datastar::events([
            kit::Confirm::close().into(),
            kit::Flash::bad("aria2 would not let go of it; try again.")
                .patch()
                .into(),
        ]),
        Err(e) => {
            tracing::warn!(%gid, error = %e, "remove refused");
            askama_datastar::events([
                kit::Confirm::close().into(),
                kit::Flash::bad("aria2 could not be asked to remove it.")
                    .patch()
                    .into(),
            ])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aria2::File as Aria2File;

    /// A torrent halfway through, which most of these draw and vary one field
    /// of. Built through `Download::new` so a test cannot pin a combination
    /// the conversion would never produce.
    fn moving() -> aria2::Download {
        aria2::Download {
            gid: "abc".to_string(),
            total_length: 1_073_741_824,
            completed_length: 536_870_912,
            download_speed: 1_048_576,
            connections: 1,
            num_seeders: 1,
            bittorrent_name: Some("Show.S01E01".to_string()),
            ..aria2::Download::fixture()
        }
    }

    fn finished() -> aria2::Download {
        aria2::Download {
            completed_length: 1_073_741_824,
            download_speed: 0,
            ..moving()
        }
    }

    fn draw(d: &aria2::Download) -> String {
        draw_with(d, Vec::new(), false)
    }

    fn draw_with(d: &aria2::Download, log: Vec<Entry>, clearing: bool) -> String {
        Download::new(d, log, clearing, Duration::from_millis(100))
            .draw()
            .as_str()
            .to_string()
    }

    fn file(
        index: u32,
        path: &str,
        length: u64,
        completed_length: u64,
        selected: bool,
    ) -> Aria2File {
        Aria2File {
            index,
            path: path.to_string(),
            length,
            completed_length,
            selected,
        }
    }

    /// The collapsed half of a row.
    fn summary(html: &str) -> &str {
        let start = html.find("<summary").expect("a row has a summary");
        let end = html.find("</summary>").expect("a row has a summary");
        &html[start..end]
    }

    /// A stream compares a row by its fingerprint, so a download drawn twice
    /// from the same state must agree, or every pass would send it.
    #[test]
    fn a_download_drawn_twice_has_one_fingerprint() {
        for d in [moving(), finished()] {
            assert_eq!(fingerprint(&draw(&d)), fingerprint(&draw(&d)));
        }
    }

    /// The marker comes off a resolving magnet's name, and nothing about the
    /// row may claim the torrent arrived with its metadata: the
    /// metadata pass reaches `Complete` when the torrent file arrives.
    #[test]
    fn a_resolved_metadata_pass_still_reads_as_a_torrent_that_has_not_started() {
        let html = draw(&aria2::Download {
            status: Status::Complete,
            bittorrent_name: Some("[METADATA]Show.S01E01".to_string()),
            total_length: 34_000,
            completed_length: 34_000,
            ..moving()
        });
        assert!(html.contains("Show.S01E01") && !html.contains("[METADATA]"));
        assert!(html.contains("finding peers"), "{html}");
        assert!(!html.contains("status ok"), "{html}");
        assert!(!html.contains("value=\""), "the bar claimed a percentage");
    }

    /// The two numbers that answer "will this be done tonight" sit in the
    /// collapsed row, as a transfer the page moves between readings.
    #[test]
    fn the_collapsed_row_shows_the_rate_and_the_time_left() {
        let html = draw(&moving());
        let summary = summary(&html);
        assert!(summary.contains("data-kit-transfer"), "{summary}");
        assert!(summary.contains("512.0 MB</span> of 1.0 GB"), "{summary}");
        assert!(summary.contains("1.0 MB/s"), "no rate: {summary}");
        assert!(summary.contains("8m 32s left"), "no time left: {summary}");
    }

    /// Nothing is moving, so there is no rate to show and nothing to divide
    /// the remaining bytes by, and the transfer stands still.
    #[test]
    fn an_idle_row_shows_neither_and_keeps_the_dash_in_the_readings() {
        let row = Download::new(&finished(), Vec::new(), false, Duration::from_millis(100));
        assert_eq!(row.rate, 0, "an idle row would move");
        let html = row.draw().as_str().to_string();
        assert!(
            !summary(&html).contains("B/s"),
            "an idle row still shows a rate: {html}"
        );
        assert!(html.contains("—"), "the readings lost their dash: {html}");
    }

    /// A hash check's bytes are hashed, not downloaded, so the bar follows
    /// them without moving them at the download's rate.
    #[test]
    fn a_hash_check_stands_still_between_readings() {
        let d = aria2::Download {
            verified_length: Some(268_435_456),
            ..moving()
        };
        let row = Download::new(&d, Vec::new(), false, Duration::from_millis(100));
        assert_eq!((row.bytes, row.rate), (268_435_456, 0));
    }

    /// A row moves a reading for as long as the next takes to arrive: the
    /// poll's sleep and the round trip that brought it.
    #[test]
    fn a_reading_moves_until_the_next_is_due() {
        let d = aria2::Download {
            read_in: Duration::from_millis(40),
            ..moving()
        };
        let row = Download::new(&d, Vec::new(), false, Duration::from_millis(100));
        assert_eq!(row.every, Duration::from_millis(140));
    }

    /// A resolving magnet has no size to move towards, but its rate still
    /// tells a working one from a stuck one, and no bytes are drawn as
    /// progress.
    #[test]
    fn a_resolving_magnet_waits_and_says_what_it_is_fetching() {
        let d = aria2::Download {
            total_length: 0,
            completed_length: 34_000,
            download_speed: 12_288,
            ..moving()
        };
        let row = Download::new(&d, Vec::new(), false, Duration::from_millis(100));
        assert_eq!((row.total, row.rate), (0, 12_288));
        let html = row.draw().as_str().to_string();
        assert!(summary(&html).contains("12.0 KB/s"), "{html}");
        assert!(
            !summary(&html).contains("33.2 KB"),
            "bytes drawn as progress: {html}"
        );
    }

    #[test]
    fn every_row_shows_its_destination() {
        let html = draw(&aria2::Download {
            dir: "/mnt/kontent/tv/Show/S01".to_string(),
            ..moving()
        });
        assert!(summary(&html).contains("/mnt/kontent/tv/Show/S01"));
    }

    #[test]
    fn the_expanded_row_carries_the_readings_a_percentage_cannot() {
        let html = draw(&aria2::Download {
            upload_length: 268_435_456,
            piece_length: 4_194_304,
            num_pieces: 256,
            info_hash: Some("d1e2f3a4b5c6".to_string()),
            ..moving()
        });
        for reading in ["256.0 MB", "0.50", "256 × 4.0 MB", "d1e2f3a4b5c6"] {
            assert!(html.contains(reading), "no {reading}: {html}");
        }
    }

    /// aria2 often leaves `errorMessage` empty; the code is always there.
    #[test]
    fn a_failure_with_no_message_still_explains_itself_from_the_code() {
        let html = draw(&aria2::Download {
            status: Status::Error,
            error_message: Some(String::new()),
            error_code: Some(9),
            ..moving()
        });
        assert!(html.contains("notice bad"), "no error notice: {html}");
        assert!(html.contains("aria2 code 9 — not enough disk space"));
        assert!(html.contains("gid abc"), "{html}");
    }

    #[test]
    fn a_healthy_row_has_no_error_notice() {
        assert!(!draw(&moving()).contains("notice bad"));
    }

    #[test]
    fn the_file_list_shows_what_was_kept_and_what_the_filter_refused() {
        let html = draw(&aria2::Download {
            files: vec![
                file(1, "/m/Nightfall/PACKGRP.nfo", 473, 0, false),
                file(2, "/m/Nightfall/Nightfall.Harbour.mp4", 1000, 500, true),
                file(3, "/m/Nightfall/Other/COVERART.jpg", 53_226, 0, false),
            ],
            ..moving()
        });
        assert!(html.contains("keeping 1 of 3"), "{html}");
        assert!(html.contains("2 skipped"), "{html}");
        assert!(html.contains("PACKGRP.nfo") && html.contains("COVERART.jpg"));
        assert!(html.contains("Nightfall.Harbour.mp4"));
    }

    #[test]
    fn a_very_long_file_list_is_capped_and_says_how_many_it_left_out() {
        let files = (1..=40)
            .map(|i| file(i, &format!("/m/track{i}.flac"), 100, 0, true))
            .collect();
        let html = draw(&aria2::Download { files, ..moving() });
        assert!(html.contains("track24.flac"));
        assert!(!html.contains("track25.flac"));
        assert!(html.contains("and 16 more"), "{html}");
    }

    /// A log says when, in the reader's zone: the kit's `Time` carries UTC as
    /// its text and the instant in `data-at` for the browser to rewrite.
    #[test]
    fn a_log_line_is_stamped_with_a_clock_time_the_browser_localises() {
        let html = draw_with(
            &moving(),
            vec![
                Entry {
                    at: std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_754_646_731),
                    message: "added".to_string(),
                },
                Entry {
                    at: std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_754_646_853),
                    message: "keeping 1 of 3 files".to_string(),
                },
            ],
            false,
        );
        assert!(html.contains(">09:52:11<"), "no UTC fallback: {html}");
        assert!(html.contains(r#"data-at="1754646731000""#), "{html}");
        assert!(html.find("added").unwrap() < html.find("keeping 1 of 3").unwrap());
    }

    /// A download already in aria2 when this pod started has no history here,
    /// and an empty panel would read as the history having been lost.
    #[test]
    fn a_download_with_no_history_shows_no_log_panel() {
        assert!(!draw(&moving()).contains("log-abc"));
    }

    /// A finished download is one press from leaving the list and the press
    /// keeps every file, so it says "clear" and asks nothing; an unfinished
    /// one deletes what it wrote, so it says so and asks first.
    #[test]
    fn clearing_keeps_a_finished_download_and_asks_before_deleting_an_unfinished_one() {
        let done = draw(&finished());
        assert!(
            done.contains(">clear<") || done.contains("clear</"),
            "{done}"
        );
        assert!(!done.contains("data-ask"), "{done}");

        let unfinished = draw(&moving());
        assert!(unfinished.contains("cancel &#38; delete"), "{unfinished}");
        assert!(unfinished.contains("data-ask"), "{unfinished}");
    }

    /// A row offers each control once, which only counting sees: a block
    /// nested inside its own branch would ship two Pause buttons.
    #[test]
    fn a_row_offers_each_control_once() {
        for d in [
            finished(),
            moving(),
            aria2::Download {
                status: Status::Paused,
                ..moving()
            },
        ] {
            let html = draw(&d);
            for control in ["/pause\"", "/resume\"", "/remove\""] {
                assert!(
                    html.matches(control).count() <= 1,
                    "a row repeats {control}: {html}"
                );
            }
            assert_eq!(html.matches("/remove\"").count(), 1, "{html}");
        }
    }

    /// While aria2 is asked to let go, the row says so and offers nothing to
    /// press.
    #[test]
    fn a_row_being_cleared_says_so_and_offers_nothing_to_press() {
        let html = draw_with(&finished(), Vec::new(), true);
        assert!(html.contains("clearing"), "{html}");
        assert!(!html.contains("data-url"), "{html}");
        assert!(!html.contains("status ok"), "{html}");
    }

    #[test]
    fn an_empty_list_says_so_and_an_unreachable_service_is_not_an_empty_list() {
        let empty = List::of(&[]).draw();
        assert!(empty.as_str().contains("nothing downloading"), "{empty:?}");
        let down = List::unreachable().draw();
        assert!(down.as_str().contains("unreachable"), "{down:?}");
        assert!(!down.as_str().contains("nothing downloading"));
    }
}
