//! mariastew — paste a magnet on a phone, and later open Infuse and watch it.
//!
//! A small service in front of aria2. aria2 does the downloading and holds all
//! the state; this renders it and takes instructions. Nothing is persisted on
//! this side, so there is no database and a restarted pod simply re-reads
//! reality — except for the one thing aria2 cannot restore on its own, an add
//! that was still in flight when the pod went away (see `resume`).

mod activity;
mod aria2;
mod auth;
mod config;
mod error;
mod filter;
mod notify;
mod poll;
mod resume;
mod routes;
mod state;
mod views;

use std::sync::{Arc, LazyLock};

#[cfg(test)]
use std::{fs, path::Path};

use askama_datastar::{Asset, Csp, SecurityHeaders};
use axum::Router;
use axum::routing::get;

use crate::config::Config;
use crate::state::AppState;

/// Datastar's address: askama_datastar's vendored copy, named by a hash of its
/// contents, which `askama_datastar::script::serve` lets a browser keep for good.
pub static DATASTAR_JS_URL: LazyLock<String> =
    LazyLock::new(|| askama_datastar::script::url("/assets/datastar.js"));

/// Everything this page needs comes from this origin, so the policy says so.
/// Nothing is fetched from a CDN and no handler is written into the markup —
/// the script bundle, Datastar included, is served from here, and
/// every behaviour hangs off a `data-*` attribute rather than an `onclick`.
///
/// `unsafe-eval` is the one concession, and it is not optional: Datastar
/// compiles each `data-*` expression with `Function()`, so without it no
/// attribute on any page does anything. It buys an attacker nothing on its
/// own — `script-src 'self'` still refuses to load script from anywhere else —
/// but it does mean escaping is the only thing standing between a string that
/// reaches the DOM and code that runs. Askama escapes by default, and the
/// escaping guard in `views`' tests checks every answer escapes text once.
fn csp() -> Csp {
    Csp::new().datastar()
}

/// The stylesheet as `just css` builds it, naming the typeface by a path
/// relative to itself.
const APP_CSS_SOURCE: &str = include_str!("../assets/app.css");

/// Beside the stylesheet because @radiosilence/koan-kit-web names it by a
/// relative URL; `just css` copies it out of the package.
static FONT: Asset = Asset::new(include_bytes!("../assets/geist-mono.woff2"), "font/woff2");

/// The stylesheet as served: its `@font-face` names the typeface by the
/// fingerprinted address it is cached under, so a new typeface reaches a
/// browser holding the old stylesheet's.
static APP_CSS: LazyLock<Asset> = LazyLock::new(|| {
    let css = APP_CSS_SOURCE.replace(
        "url(geist-mono.woff2)",
        &format!("url({})", FONT.url("/assets/geist-mono.woff2")),
    );
    Asset::new(css.leak().as_bytes(), "text/css; charset=utf-8")
});

static APP_JS: Asset = Asset::new(
    include_bytes!("../assets/app.js"),
    "text/javascript; charset=utf-8",
);
static MANIFEST: Asset = Asset::new(
    include_bytes!("../assets/manifest.webmanifest"),
    "application/manifest+json",
);
static ICON_SVG: Asset = Asset::new(include_bytes!("../assets/icon.svg"), "image/svg+xml");
static FAVICON: Asset = Asset::new(include_bytes!("../assets/favicon.ico"), "image/x-icon");
static APPLE_TOUCH_ICON: Asset = Asset::new(
    include_bytes!("../assets/apple-touch-icon.png"),
    "image/png",
);
static ICON_192: Asset = Asset::new(include_bytes!("../assets/icon-192.png"), "image/png");
static ICON_512: Asset = Asset::new(include_bytes!("../assets/icon-512.png"), "image/png");
static ICON_192_MASKABLE: Asset = Asset::new(
    include_bytes!("../assets/icon-192-maskable.png"),
    "image/png",
);
static ICON_512_MASKABLE: Asset = Asset::new(
    include_bytes!("../assets/icon-512-maskable.png"),
    "image/png",
);

/// Served from the binary rather than a volume: the stylesheet and the script
/// bundle are build outputs, so a mount would be a second thing to keep in
/// step with the image.
///
/// The page names each by `asset_url`, which carries a hash of its bytes, so a
/// browser keeps it for good and a deploy's page fetches the new copy. Asked
/// for by a bare path, as the manifest names its icons and a browser asks for
/// `/favicon.ico`, it is cached for an hour.
///
/// `/favicon.ico` sits beside its `/assets` copy because that is the path a
/// browser asks for on its own, before it has parsed any `<link>` — and the
/// only router that would otherwise answer it is behind the session layer, so
/// an unauthenticated tab would get a login redirect where it wanted an image.
fn assets() -> [(&'static str, &'static Asset); 12] {
    [
        ("/assets/app.js", &APP_JS),
        ("/assets/app.css", &*APP_CSS),
        ("/assets/geist-mono.woff2", &FONT),
        ("/assets/manifest.webmanifest", &MANIFEST),
        ("/assets/icon.svg", &ICON_SVG),
        ("/assets/favicon.ico", &FAVICON),
        ("/favicon.ico", &FAVICON),
        ("/assets/apple-touch-icon.png", &APPLE_TOUCH_ICON),
        ("/assets/icon-192.png", &ICON_192),
        ("/assets/icon-512.png", &ICON_512),
        ("/assets/icon-192-maskable.png", &ICON_192_MASKABLE),
        ("/assets/icon-512-maskable.png", &ICON_512_MASKABLE),
    ]
}

/// The fingerprinted address of the asset served at `path`, for the page's
/// `href` and `src`.
///
/// # Panics
///
/// If nothing is served at `path`: the templates name these, so it is the
/// program's own mistake, and rendering the page in any test finds it.
pub fn asset_url(path: &str) -> String {
    let (path, asset) = assets()
        .into_iter()
        .find(|(served, _)| *served == path)
        .unwrap_or_else(|| panic!("no asset is served at {path}"));
    asset.url(path)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    // The compile-time gate on `/auth/dev-login` (see `auth::routes::router`)
    // means this only fires in a binary that was never meant to be deployed —
    // `cargo build --release` cannot produce one. Loud, at the top of the
    // logs, so a debug binary running anywhere announces it before anything
    // else does.
    #[cfg(debug_assertions)]
    tracing::warn!(
        "DEBUG BUILD: /auth/dev-login is compiled in and skips sign-in entirely — this build must never run in production"
    );

    let config = Config::from_env()?;
    let bind_addr = config.bind_addr.clone();
    // Every outbound call has a ceiling: the notifier awaits Telegram inline,
    // so one request that never answered would stop every later notification.
    // aria2 calls set a tighter one of their own. No proxy, because the one
    // client also carries the RPC to aria2 over loopback, which a proxy
    // variable in the pod's environment would otherwise reroute.
    let http = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(30))
        .no_proxy()
        .build()?;

    let state = AppState {
        aria2: aria2::Aria2::new(config.aria2_rpc_url.clone(), http.clone()),
        config: Arc::new(config),
        sessions: auth::session::Sessions::new(),
        activity: activity::Activity::new(),
        poll: poll::Poll::new(),
        clearing: state::Clearing::default(),
        adding: state::Adding::default(),
        preexisting: Default::default(),
        http,
    };

    // The poller before the watcher: the watcher reads its snapshots and would
    // otherwise sit on an empty channel until the first tick anyway. `resume`
    // reads the same channel, and only its first snapshot.
    poll::spawn(state.clone());
    notify::spawn_watcher(state.clone());
    resume::spawn(state.clone());

    // Only what is genuinely public sits outside the auth layer: the probe the
    // kubelet calls, the login round-trip that cannot require a session to
    // establish one, and the static assets. Everything else is inside
    // `routes::router`, which the layer wraps whole.
    let mut app = Router::new()
        .merge(routes::router().layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::extract::require_session,
        )))
        .merge(auth::routes::router())
        .route("/healthz", get(|| async { "ok" }))
        .route("/assets/datastar.js", get(askama_datastar::script::serve));
    for (path, asset) in assets() {
        app = app.route(path, get(asset.serve()));
    }
    let app = app
        .layer(SecurityHeaders::new(&csp()))
        // The default predicate leaves `text/event-stream` alone: an encoder
        // buffers, and the live stream must reach the browser as it is written.
        .layer(tower_http::compression::CompressionLayer::new())
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state.clone());

    let term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!(%bind_addr, "listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown(term, state.poll.clone()))
        .await?;

    // A removal runs detached from its request, and one cut off between
    // stopping the download and deleting its partial files leaves files that
    // nothing will find again. `resume` covers an interrupted add; nothing
    // covers this, so it is given a moment to finish.
    let deadline = tokio::time::Instant::now() + CLEARING_GRACE;
    while !state.clearing.is_empty() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    Ok(())
}

/// How long shutdown waits for removals already under way.
const CLEARING_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// SIGTERM is what Kubernetes sends, and as PID 1 in the container the process
/// has no default handler for it; Ctrl-C is for a terminal. In-flight
/// requests are finished and every stream is aborted, since a stream never
/// finishes on its own and an aborted one is retried within a second, against
/// the new pod.
async fn shutdown(mut term: tokio::signal::unix::Signal, poll: poll::Poll) {
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
    tracing::info!("shutting down");
    poll.close();
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header;

    #[tokio::test]
    async fn every_response_carries_the_security_headers() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let app: Router = Router::new()
            .route("/", get(|| async { "hi" }))
            .layer(SecurityHeaders::new(&csp()));

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let headers = response.headers();
        let csp = headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
        assert!(csp.contains("default-src 'self'"));
        assert!(csp.contains("frame-ancestors 'none'"));
        assert!(
            csp.contains("'unsafe-eval'"),
            "Datastar needs the eval hatch"
        );
        // Datastar cannot work without the eval hatch, so that one is stated
        // and explained above. `unsafe-inline` is the one nothing here needs,
        // and it is what someone reaches for to make a stray inline handler
        // work — which is exactly the change this should refuse.
        assert!(
            !csp.contains("unsafe-inline"),
            "csp has an inline escape hatch"
        );
        assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(headers[header::X_FRAME_OPTIONS], "DENY");
        assert_eq!(headers[header::REFERRER_POLICY], "same-origin");
    }

    /// The stylesheet loads the typeface by the address it is cached under.
    #[tokio::test]
    async fn the_stylesheet_names_the_fingerprinted_typeface() {
        assert!(
            APP_CSS_SOURCE.contains("url(geist-mono.woff2)"),
            "the stylesheet no longer names the typeface this way; the rewrite in APP_CSS finds nothing"
        );
        let body = APP_CSS.response(None).into_body();
        let css = axum::body::to_bytes(body, usize::MAX).await.unwrap();
        let css = std::str::from_utf8(&css).unwrap();
        assert!(css.contains(&format!("url({})", FONT.url("/assets/geist-mono.woff2"))));
        assert!(!css.contains("url(geist-mono.woff2)"));
    }

    /// With eval allowed, escaping is the only thing between a string that
    /// reaches the DOM and code that runs — a `data-*` attribute built from an
    /// unescaped value is executable by design.
    ///
    /// Askama escapes by default, so what needs watching is the opt-outs. The
    /// one below is a sub-template's already-rendered markup rather than a
    /// value, which is the only reason it is allowed to be raw. A new one is
    /// not necessarily wrong, but it has to be looked at — so it lands here.
    #[test]
    fn escaping_is_only_opted_out_of_where_expected() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("templates");
        let mut found = Vec::new();
        for entry in fs::read_dir(&dir).expect("templates/") {
            let path = entry.expect("entry").path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            for line in fs::read_to_string(&path).expect("readable").lines() {
                if line.contains("|safe") || line.contains("escape(") {
                    found.push(format!("{name}: {}", line.trim()));
                }
            }
        }
        found.sort();
        assert_eq!(found, ["page.html: {{ self.picker()?|safe }}"]);
    }
}
