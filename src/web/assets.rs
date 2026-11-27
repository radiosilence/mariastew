//! The files the page loads, served from the binary rather than a volume: the
//! stylesheet is a build output, so a mount would be a second thing to keep in
//! step with the image.
//!
//! The page names each by [`url`], which carries a hash of its bytes, so a
//! browser keeps it for good and a deploy's page fetches the new copy. Asked
//! for by a bare path, as the manifest names its icons and a browser asks for
//! `/favicon.ico`, it is cached for an hour. All of them sit outside the
//! session layer: the sign-in page needs an icon too, and a manifest is fetched
//! without credentials, so one behind the layer would answer an install prompt
//! with a login redirect.

use askama_datastar::Asset;
use axum::Router;
use axum::routing::get;
use koan_kit_web as kit;

use crate::state::AppState;

static APP_CSS: Asset = Asset::new(
    include_bytes!("../../assets/app.css"),
    "text/css; charset=utf-8",
);
static MANIFEST: Asset = Asset::new(
    include_bytes!("../../assets/manifest.webmanifest"),
    "application/manifest+json",
);
static ICON_SVG: Asset = Asset::new(include_bytes!("../../assets/icon.svg"), "image/svg+xml");
static FAVICON: Asset = Asset::new(include_bytes!("../../assets/favicon.ico"), "image/x-icon");
static APPLE_TOUCH_ICON: Asset = Asset::new(
    include_bytes!("../../assets/apple-touch-icon.png"),
    "image/png",
);
static ICON_192: Asset = Asset::new(include_bytes!("../../assets/icon-192.png"), "image/png");
static ICON_512: Asset = Asset::new(include_bytes!("../../assets/icon-512.png"), "image/png");
static ICON_192_MASKABLE: Asset = Asset::new(
    include_bytes!("../../assets/icon-192-maskable.png"),
    "image/png",
);
static ICON_512_MASKABLE: Asset = Asset::new(
    include_bytes!("../../assets/icon-512-maskable.png"),
    "image/png",
);

/// Every embedded file and the path it is served at. `/favicon.ico` sits
/// beside its `/assets` copy because that is the path a browser asks for on
/// its own, before it has parsed any `<link>`.
static SERVED: [(&str, &Asset); 10] = [
    ("/assets/app.css", &APP_CSS),
    ("/assets/manifest.webmanifest", &MANIFEST),
    ("/assets/icon.svg", &ICON_SVG),
    ("/assets/favicon.ico", &FAVICON),
    ("/favicon.ico", &FAVICON),
    ("/assets/apple-touch-icon.png", &APPLE_TOUCH_ICON),
    ("/assets/icon-192.png", &ICON_192),
    ("/assets/icon-512.png", &ICON_512),
    ("/assets/icon-192-maskable.png", &ICON_192_MASKABLE),
    ("/assets/icon-512-maskable.png", &ICON_512_MASKABLE),
];

/// The assets, Datastar as askama_datastar vendors it, and the kit's typeface
/// beside the stylesheet, where the stylesheet asks for it.
pub(super) fn router() -> Router<AppState> {
    let mut router = Router::new()
        .route("/assets/datastar.js", get(askama_datastar::script::serve))
        .route("/assets/geist-mono.woff2", get(kit::font::serve));
    for (path, asset) in &SERVED {
        router = router.route(path, get(asset.serve()));
    }
    router
}

/// The fingerprinted address of the asset served at `path`, for the page's
/// `href` and `src`.
///
/// # Panics
///
/// If nothing is served at `path`: the templates name these, so it is the
/// program's own mistake, and drawing the page in any test finds it.
pub(super) fn url(path: &str) -> String {
    let (path, asset) = SERVED
        .iter()
        .find(|(served, _)| *served == path)
        .unwrap_or_else(|| panic!("no asset is served at {path}"));
    asset.url(path)
}
