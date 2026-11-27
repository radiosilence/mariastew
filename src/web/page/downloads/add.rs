//! The add sheet: one or more magnet rows and the destination they land in.
//!
//! The sheet (`kit::Sheet::loads`, drawn by the page) reads its body from
//! `GET /add` each time it opens, so every attempt starts fresh however the
//! last one was abandoned. Adding answers with as little as it can. A batch
//! that went in whole closes the sheet (`kit::Sheet::dismiss`), and the stream
//! draws the new rows behind it, which is the confirmation. A batch refused or
//! partly taken answers with the sheet's two regions it changed: `#add-error`
//! saying why, and `#magnets` holding only the magnets that did not go in, so
//! pressing add again sends only those.

use askama::Template;
use askama_datastar::Html;
use axum::extract::State;
use axum::response::Response;
use axum_extra::extract::Form;
use koan_kit_web as kit;

use super::destination::Destination;
use crate::add::{self, Refused};
use crate::auth::extract::CurrentSession;
use crate::state::AppState;
use crate::web::Draw;

/// The sheet's body, which `#add` reads as it opens.
#[derive(Template)]
#[template(path = "downloads/add.html")]
pub(in crate::web) struct Body {
    magnets: Html,
    error: Html,
    destination: Html,
}

impl Body {
    fn of(state: &AppState) -> Self {
        Self {
            magnets: Magnets::blank().draw(),
            error: Error::none().draw(),
            destination: Destination::roots(state).draw(),
        }
    }
}

/// The magnet rows, `#magnets`, a `kit::RepeatField`: one row per magnet, or
/// one blank row for none.
#[derive(Template)]
#[template(path = "downloads/add/magnets.html")]
pub(in crate::web) struct Magnets {
    magnets: Vec<String>,
}

impl Magnets {
    fn blank() -> Self {
        Self {
            magnets: Vec::new(),
        }
    }
}

/// Why the last press added nothing, or not everything: `#add-error`, inside
/// the sheet, since a flash would sit under the dimmed page.
#[derive(Template)]
#[template(path = "downloads/add/error.html")]
pub(in crate::web) struct Error {
    message: Option<String>,
}

impl Error {
    fn none() -> Self {
        Self { message: None }
    }

    pub(in crate::web) fn of(message: impl Into<String>) -> Self {
        Self {
            message: Some(message.into()),
        }
    }
}

/// The sheet's body as a fresh sheet has it: one blank row, no error, the
/// destinations at the top.
pub(in crate::web) async fn fresh(State(state): State<AppState>) -> Response {
    match kit::Markup::render(&Body::of(&state)) {
        Ok(body) => askama_datastar::events([kit::Sheet::fill("add", body).into()]),
        Err(e) => crate::web::failed(e),
    }
}

/// One `magnet` per row, so the field repeats, which axum's own `Form` cannot
/// read into a list.
#[derive(serde::Deserialize)]
pub(in crate::web) struct AddForm {
    #[serde(default, rename = "magnet")]
    magnets: Vec<String>,
    #[serde(default)]
    dir: String,
}

/// What to say when some of a batch did not go in. A batch of one says what a
/// single add always has.
fn refused_message(total: usize, refused: &[Refused]) -> String {
    if let [only] = refused
        && total == 1
    {
        return match only {
            Refused::AlreadyListed => "That is already in the list.",
            Refused::Unreachable => "The download service could not be reached.",
        }
        .to_string();
    }
    let count = |r| refused.iter().filter(|&&x| x == r).count();
    let mut reasons = Vec::new();
    match count(Refused::AlreadyListed) {
        0 => {}
        1 => reasons.push("1 is already in the list".to_string()),
        n => reasons.push(format!("{n} are already in the list")),
    }
    match count(Refused::Unreachable) {
        0 => {}
        n => reasons.push(format!("{n} could not reach the download service")),
    }
    format!(
        "Added {} of {total}; {}.",
        total - refused.len(),
        reasons.join(", ")
    )
}

/// Validates the batch whole, then starts each magnet (`add::start`).
/// Validation is all or nothing, because nothing has been added when it fails
/// and the row at fault is the one to fix; starting is per magnet, since one
/// already in the list is no reason to refuse the others.
pub(in crate::web) async fn add(
    State(state): State<AppState>,
    CurrentSession(session): CurrentSession,
    Form(form): Form<AddForm>,
) -> Response {
    let refuse =
        |message: &str| askama_datastar::events([Error::of(message).draw().patch().into()]);
    // Blank rows are rows someone added and never filled, not mistakes.
    let rows: Vec<(usize, &str)> = form
        .magnets
        .iter()
        .map(|m| m.trim())
        .enumerate()
        .filter(|(_, m)| !m.is_empty())
        .collect();
    if rows.is_empty() {
        return refuse("Paste a magnet link.");
    }
    if let Some((row, _)) = rows.iter().find(|(_, m)| !add::is_magnet(m)) {
        return if form.magnets.len() == 1 {
            refuse("That is not a magnet link.")
        } else {
            refuse(&format!("Link {} is not a magnet link.", row + 1))
        };
    }
    let Some(dir) = state.config.resolve(&form.dir) else {
        return refuse("Choose a destination inside the library.");
    };
    let dir = dir.to_string_lossy();

    let mut left = Vec::new();
    let mut refused = Vec::new();
    for &(_, magnet) in &rows {
        if let Err(r) = add::start(&state, &session.sub, magnet, &dir).await {
            left.push(magnet.to_string());
            refused.push(r);
        }
    }

    if refused.is_empty() {
        return askama_datastar::events([kit::Sheet::dismiss("add").into()]);
    }
    askama_datastar::events([
        Error::of(refused_message(rows.len(), &refused))
            .draw()
            .patch()
            .into(),
        Magnets { magnets: left }.draw().patch().into(),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::add::tests::{MetadataOutcome, mock_server, state};
    use crate::auth::session::Session;

    fn session() -> Session {
        Session {
            id: "s".to_string(),
            sub: "tester".to_string(),
            expires: std::time::Instant::now() + std::time::Duration::from_secs(60),
        }
    }

    async fn answer(state: AppState, magnets: &[&str]) -> String {
        let form = AddForm {
            magnets: magnets.iter().map(|m| m.to_string()).collect(),
            dir: "/mnt/kontent/movies".to_string(),
        };
        let response = add(State(state), CurrentSession(session()), Form(form)).await;
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        String::from_utf8(body.to_vec()).unwrap()
    }

    /// A batch is checked whole before anything is added: a row that is not a
    /// magnet refuses the lot, by its row number, and aria2 never hears about
    /// the rows around it.
    #[tokio::test]
    async fn a_batch_with_one_bad_row_adds_nothing_and_names_the_row() {
        let mock = mock_server(
            serde_json::json!({"result": "metadata-gid"}),
            MetadataOutcome::NeverResolves,
        )
        .await;
        let calls = mock.calls.clone();
        let answer = answer(
            state(mock.url),
            &[
                "magnet:?xt=urn:btih:abc",
                "",
                "https://example.com/x.torrent",
            ],
        )
        .await;
        assert!(answer.contains("Link 3 is not a magnet link."), "{answer}");
        assert!(answer.contains("add-error"), "{answer}");
        assert!(calls.lock().unwrap().is_empty(), "aria2 was asked anyway");
    }

    /// The magnets aria2 would not take stay in the sheet, and nothing else
    /// does, so pressing add again sends only those.
    #[tokio::test]
    async fn a_partly_refused_batch_leaves_only_the_refused_magnets() {
        let mock = mock_server(
            serde_json::json!({"error": {"code": 1, "message": "InfoHash abc is already registered."}}),
            MetadataOutcome::NeverResolves,
        )
        .await;
        let answer = answer(
            state(mock.url),
            &["magnet:?xt=urn:btih:abc", "magnet:?xt=urn:btih:def"],
        )
        .await;
        assert!(
            answer.contains("Added 0 of 2; 2 are already in the list."),
            "{answer}"
        );
        assert!(answer.contains("id=\"magnets\""), "{answer}");
        assert!(answer.contains("btih:def"), "{answer}");
    }

    /// A batch that went in whole closes the sheet and sends nothing else: the
    /// stream draws the new rows.
    #[tokio::test]
    async fn a_batch_that_went_in_whole_closes_the_sheet() {
        let mock = mock_server(
            serde_json::json!({"result": "metadata-gid"}),
            MetadataOutcome::NeverResolves,
        )
        .await;
        let answer = answer(state(mock.url), &["magnet:?xt=urn:btih:abc"]).await;
        assert!(answer.contains("#add > section"), "{answer}");
        assert!(!answer.contains("add-error"), "{answer}");
    }

    #[test]
    fn a_partly_refused_batch_says_how_much_went_in_and_why_the_rest_did_not() {
        assert_eq!(
            refused_message(1, &[Refused::AlreadyListed]),
            "That is already in the list."
        );
        assert_eq!(
            refused_message(4, &[Refused::AlreadyListed, Refused::Unreachable]),
            "Added 2 of 4; 1 is already in the list, 1 could not reach the download service."
        );
    }
}
