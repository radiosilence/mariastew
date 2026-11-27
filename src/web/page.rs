//! The pages, one module each, named as the UI names them. There is one: the
//! downloads ([`downloads`]).
//!
//! [`serve`] draws it two ways. A browser asking for the address gets the
//! document around the body. The page itself asking, through a link or form
//! that loads in place, gets the body morphed into `main` and the address as
//! `$url` (`askama_datastar::address`), which the document's
//! `askama_datastar::history("url")` keeps the address bar on. So a reload, a
//! shared link, back and forward all draw what the address names.

use axum::extract::{OriginalUri, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};

use super::Draw;
use super::layout::{Layout, Main};
use super::stream::downloads::Seen;
use crate::error::AppError;
use crate::state::AppState;

pub(super) mod downloads;

/// The signals a page starts with. Every page declares every signal the
/// stream's `reopen_on` names (`url`), and what its stream carries on from
/// (`seen`): the rows the body drew, which the stream does not send again.
fn signals(url: &str, seen: &Seen) -> serde_json::Value {
    serde_json::json!({ "url": url, "seen": seen })
}

/// Draw the downloads page: whole for a browser, in place for the page.
pub(super) async fn serve(
    State(state): State<AppState>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Response {
    let url = askama_datastar::address(&uri);
    let (page, seen) = downloads::Page::of(&state).await;
    let main = Main { body: page.draw() }.draw();
    if askama_datastar::is_datastar(&headers) {
        return askama_datastar::events([
            main.patch().into(),
            askama_datastar::PatchSignals::new(signals(&url, &seen).to_string()).into(),
        ]);
    }
    let mut signals = signals(&url, &seen);
    if let Some(all) = signals.as_object_mut() {
        // The add sheet's own, which the server sets and the sheet reads.
        all.extend(downloads::add::signals());
    }
    match askama_datastar::Html::render(&Layout {
        signals: signals.to_string(),
        main,
    }) {
        Ok(html) => html.into_response(),
        Err(e) => AppError::Internal(e.into()).into_response(),
    }
}
