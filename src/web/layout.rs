//! The document every answer to a page load is drawn in: the kit's head, the
//! live stream's element, the page's own region, the footer, and the page's
//! one question sheet and flash.

use askama::Template;
use askama_datastar::Html;
use koan_kit_web as kit;

/// The document, around the page's own region.
#[derive(Template)]
#[template(path = "layout.html")]
pub(super) struct Layout {
    /// The page's starting signals, as JSON.
    pub(super) signals: String,
    pub(super) main: Html,
}

impl Layout {
    /// The build serving this page: `Cargo.toml`'s version is what a release
    /// is here, so it is the same string the image tag and the GitHub release
    /// carry, neither of which is visible from a phone.
    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn asset(&self, path: &str) -> String {
        super::assets::url(path)
    }

    /// The address bar following `$url`, the address a page loaded in place
    /// shows.
    fn history(&self) -> Html {
        askama_datastar::history("url")
    }

    /// The live stream, opened again whenever it ends or goes quiet, and
    /// afresh when the address changes. Every signal named here is declared
    /// by every page (`page::signals`): an undeclared one stops the stream
    /// reopening without a console message.
    fn stream(&self) -> Html {
        askama_datastar::reopen_on("/stream/downloads", &["url"])
    }
}

/// The page's own region, which an in-place answer replaces whole.
#[derive(Template)]
#[template(path = "layout/main.html")]
pub(super) struct Main {
    pub(super) body: Html,
}
