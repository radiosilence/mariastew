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

use std::sync::Arc;

#[cfg(test)]
use std::{fs, path::Path};

use axum::Router;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;

use crate::config::Config;
use crate::state::AppState;

/// `build.rs` finds the one `assets/app-<hash>.js` rolldown wrote and hands
/// its name over as an env var — the hash is how a deploy's new script stops
/// being served from a browser cache still holding the old one, and it is not
/// something the source can spell on its own since rolldown chooses it.
/// `views::Page::app_js_path` renders the same constant into the `<script>`
/// tag that requests it.
pub const APP_JS_PATH: &str = concat!("/assets/", env!("APP_JS_FILENAME"));

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
/// reaches the DOM and code that runs. Askama escapes by default and nothing
/// here opts out; `no_template_disables_escaping` is what keeps that true.
///
/// Shared verbatim with mcp-gateway, which reaches the same policy from the
/// other direction: it had this without `unsafe-eval` until it wanted Datastar.
const CSP: &str = "default-src 'self'; \
     script-src 'self' 'unsafe-eval'; \
     style-src 'self'; \
     img-src 'self' data:; \
     connect-src 'self'; \
     form-action 'self'; \
     frame-ancestors 'none'; \
     base-uri 'none'; \
     object-src 'none'";

/// Applied to every response. Harmless on the SSE and JSON the app returns, and
/// cheaper to reason about than deciding per route which ones render HTML.
async fn security_headers(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        axum::http::HeaderValue::from_static(CSP),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        axum::http::HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        axum::http::HeaderValue::from_static("no-referrer"),
    );
    // Redundant beside frame-ancestors for anything current, and free.
    headers.insert(
        header::X_FRAME_OPTIONS,
        axum::http::HeaderValue::from_static("DENY"),
    );
    response
}

/// Served from the binary rather than a volume: the stylesheet and the script
/// bundle are build outputs, so a mount would be a second thing to keep in
/// step with the image.
///
/// Bodies are bytes rather than `&str` because half of these are now PNGs.
/// `include_bytes!` covers the text ones too — the content type is stated here
/// either way, so nothing was being checked by the compiler's insistence that
/// the script was UTF-8.
///
/// `/favicon.ico` sits beside its `/assets` copy because that is the path a
/// browser asks for on its own, before it has parsed any `<link>` — and the
/// only router that would otherwise answer it is behind the session layer, so
/// an unauthenticated tab would get a login redirect where it wanted an image.
const ASSETS: [(&str, &str, &[u8]); 11] = [
    (
        APP_JS_PATH,
        "text/javascript; charset=utf-8",
        include_bytes!(concat!("../assets/", env!("APP_JS_FILENAME"))),
    ),
    (
        "/assets/app.css",
        "text/css; charset=utf-8",
        include_bytes!("../assets/app.css"),
    ),
    (
        "/assets/manifest.webmanifest",
        "application/manifest+json",
        include_bytes!("../assets/manifest.webmanifest"),
    ),
    (
        "/assets/icon.svg",
        "image/svg+xml",
        include_bytes!("../assets/icon.svg"),
    ),
    (
        "/assets/favicon.ico",
        "image/x-icon",
        include_bytes!("../assets/favicon.ico"),
    ),
    (
        "/favicon.ico",
        "image/x-icon",
        include_bytes!("../assets/favicon.ico"),
    ),
    (
        "/assets/apple-touch-icon.png",
        "image/png",
        include_bytes!("../assets/apple-touch-icon.png"),
    ),
    (
        "/assets/icon-192.png",
        "image/png",
        include_bytes!("../assets/icon-192.png"),
    ),
    (
        "/assets/icon-512.png",
        "image/png",
        include_bytes!("../assets/icon-512.png"),
    ),
    (
        "/assets/icon-192-maskable.png",
        "image/png",
        include_bytes!("../assets/icon-192-maskable.png"),
    ),
    (
        "/assets/icon-512-maskable.png",
        "image/png",
        include_bytes!("../assets/icon-512-maskable.png"),
    ),
];

/// How an asset may be cached. The script bundle's name changes with its
/// content, so it is cached for good. Everything else keeps its name across
/// deploys and is revalidated against an ETag instead, so a deploy's new
/// stylesheet is never paired with the old one from cache.
fn cache_headers(path: &str, body: &[u8]) -> (&'static str, Option<String>) {
    if path == APP_JS_PATH {
        return ("public, max-age=31536000, immutable", None);
    }
    use std::hash::{DefaultHasher, Hash as _, Hasher as _};
    let mut hasher = DefaultHasher::new();
    body.hash(&mut hasher);
    ("no-cache", Some(format!("\"{:016x}\"", hasher.finish())))
}

/// `If-None-Match` matches under weak comparison (RFC 9110 13.1.2): any entry
/// of a list, `*`, and a `W/` validator, which is what an ingress that
/// compresses the response turns a strong one into.
fn none_match(header: &axum::http::HeaderValue, etag: &str) -> bool {
    let Ok(header) = header.to_str() else {
        return false;
    };
    header.split(',').map(str::trim).any(|candidate| {
        candidate == "*" || candidate.strip_prefix("W/").unwrap_or(candidate) == etag
    })
}

async fn asset(
    headers: axum::http::HeaderMap,
    content_type: &'static str,
    cache_control: &'static str,
    etag: Option<String>,
    body: &'static [u8],
) -> Response {
    let Some(etag) = etag else {
        return (
            [
                (header::CONTENT_TYPE, content_type),
                (header::CACHE_CONTROL, cache_control),
            ],
            body,
        )
            .into_response();
    };
    let fresh = headers
        .get(header::IF_NONE_MATCH)
        .is_some_and(|v| none_match(v, &etag));
    let common = [
        (header::CACHE_CONTROL, cache_control.to_string()),
        (header::ETAG, etag),
    ];
    if fresh {
        (StatusCode::NOT_MODIFIED, common).into_response()
    } else {
        (common, [(header::CONTENT_TYPE, content_type)], body).into_response()
    }
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
        .route("/healthz", get(|| async { "ok" }));
    for (path, content_type, body) in ASSETS {
        let (cache_control, etag) = cache_headers(path, body);
        app = app.route(
            path,
            get(move |headers| asset(headers, content_type, cache_control, etag.clone(), body)),
        );
    }
    let app = app
        .layer(axum::middleware::from_fn(security_headers))
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

    #[tokio::test]
    async fn the_hashed_bundle_is_immutable_and_everything_else_revalidates() {
        let serve = |headers, path, body: &'static [u8]| {
            let (cache_control, etag) = cache_headers(path, body);
            asset(headers, "text/plain", cache_control, etag, body)
        };
        let bundle = serve(Default::default(), APP_JS_PATH, b"js").await;
        assert_eq!(
            bundle.headers()[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );
        assert!(!bundle.headers().contains_key(header::ETAG));

        let css = serve(Default::default(), "/assets/app.css", b"css").await;
        assert_eq!(css.headers()[header::CACHE_CONTROL], "no-cache");
        let etag = css.headers()[header::ETAG].to_str().unwrap().to_string();

        for sent in [
            etag.clone(),
            format!("W/{etag}"),
            format!("\"other\", {etag}"),
            "*".to_string(),
        ] {
            let mut headers = axum::http::HeaderMap::new();
            headers.insert(header::IF_NONE_MATCH, sent.parse().unwrap());
            let again = serve(headers, "/assets/app.css", b"css").await;
            assert_eq!(again.status(), StatusCode::NOT_MODIFIED, "{sent}");
        }

        let mut headers = axum::http::HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, etag.parse().unwrap());
        let changed = serve(headers, "/assets/app.css", b"new css").await;
        assert_eq!(changed.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn every_response_carries_the_security_headers() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let app = Router::new()
            .route("/", get(|| async { "hi" }))
            .layer(axum::middleware::from_fn(security_headers));

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let headers = response.headers();
        let csp = headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
        assert!(csp.contains("default-src 'self'"));
        assert!(csp.contains("frame-ancestors 'none'"));
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
        assert_eq!(headers[header::REFERRER_POLICY], "no-referrer");
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
