//! The web UI: one page, the downloads, server-rendered and patched over SSE by
//! Datastar.
//!
//! The page is a module under [`page`], drawn by its templates under
//! `templates/downloads/` inside the shared document ([`layout`]). It names a
//! few regions by id, and Datastar morphs a region whole when the server sends
//! it again: an action answering, or the page's live stream ([`stream`]),
//! which sends a region only when it changed. Markup comes from templates and
//! the kōan web kit's components alone, so every value in it is escaped exactly
//! once, where a template writes it. `ARCHITECTURE.md` describes the layout.
//!
//! Everything but sign-in, the probe and the static assets sits behind the
//! session layer, which wraps the protected router whole so a route added
//! later is protected by construction: aria2's RPC has no auth of its own, so
//! a route reaching it from outside the layer would be full control of the
//! queue and write access to the media tree.

use askama::Template;
use askama_datastar::{Csp, Html, SecurityHeaders};
use axum::Router;
use axum::response::Response;
use axum::routing::{get, post};
use koan_kit_web as kit;

use crate::state::AppState;

mod assets;
#[cfg(test)]
mod escaping;
mod layout;
mod page;
mod stream;

/// Every route the service answers, in one place: the page, its live stream,
/// and the actions the page posts to, behind the session layer; sign-in, the
/// probe and the assets outside it.
pub fn router(state: AppState) -> Router {
    use page::downloads::{self, add, destination};

    let pages = Router::new().route("/", get(page::serve));
    let streams = Router::new().route("/stream/downloads", get(stream::serve));
    let actions = Router::new()
        // The add sheet
        .route("/add", get(add::fresh).post(add::add))
        .route("/browse", get(destination::browse))
        .route("/mkdir", post(destination::mkdir))
        // A download
        .route("/downloads/{gid}/pause", post(downloads::pause))
        .route("/downloads/{gid}/resume", post(downloads::resume))
        .route("/downloads/{gid}/remove", post(downloads::remove));
    let protected = pages
        .merge(streams)
        .merge(actions)
        // A POST Datastar did not send is refused, which stops cross-site
        // forgery whatever the cookie's `SameSite` allows.
        .layer(axum::middleware::from_fn(
            askama_datastar::require_datastar_on_post,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::auth::extract::require_session,
        ));
    Router::new()
        .merge(protected)
        .merge(crate::auth::routes::router())
        .merge(assets::router())
        .route("/healthz", get(|| async { "ok" }))
        .layer(SecurityHeaders::new(&csp()))
        // The default predicate leaves `text/event-stream` alone: an encoder
        // buffers, and the live stream must reach the browser as it is written.
        .layer(tower_http::compression::CompressionLayer::new())
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state)
}

/// Everything the page needs comes from this origin. `unsafe-eval` is the one
/// concession, and Datastar cannot work without it: it compiles each `data-*`
/// expression with `Function()`. That makes escaping the only thing between a
/// string reaching the DOM and code that runs, which is why every expression
/// is a constant and the escaping guard (`web::escaping`) checks every answer.
fn csp() -> Csp {
    Csp::new().datastar()
}

/// A view rendered as markup, which another template embeds as it is:
/// everything in it was escaped as the view was rendered. A view that fails to
/// render is logged and drawn as nothing.
trait Draw: Template + Sized {
    fn draw(&self) -> Html {
        Html::render(self).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "could not render a view");
            Html::new()
        })
    }
}

impl<T: Template> Draw for T {}

/// One patch as the whole answer.
fn one(html: &Html) -> Response {
    askama_datastar::events([html.patch().into()])
}

/// An error, as the flash over the page.
fn failed(message: impl std::fmt::Display) -> Response {
    one(&kit::Flash::bad(message.to_string()).html())
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

        let response = router(AppState::fixture(String::new()))
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
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
        // `unsafe-inline` is what someone reaches for to make a stray inline
        // handler work, which is exactly the change this should refuse.
        assert!(
            !csp.contains("unsafe-inline"),
            "csp has an inline escape hatch"
        );
        assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(headers[header::X_FRAME_OPTIONS], "DENY");
        assert_eq!(headers[header::REFERRER_POLICY], "same-origin");
    }

    /// A POST Datastar did not send is refused before the session layer
    /// answers for it, so another site's form cannot drive the queue.
    #[tokio::test]
    async fn a_post_datastar_did_not_send_is_refused() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        let state = AppState::fixture(String::new());
        let session = state
            .sessions
            .create("tester", std::time::Duration::from_secs(60))
            .await;
        let response = router(state)
            .oneshot(
                Request::post("/downloads/abc/pause")
                    .header(
                        header::COOKIE,
                        format!("{}={session}", crate::auth::cookie::SESSION_COOKIE),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}
