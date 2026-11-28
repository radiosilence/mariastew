//! mariastew — paste a magnet on a phone, and later open Infuse and watch it.
//!
//! A small service in front of aria2. aria2 does the downloading and holds all
//! the state; this renders it and takes instructions. Nothing is persisted on
//! this side, so there is no database and a restarted pod simply re-reads
//! reality — except for the one thing aria2 cannot restore on its own, an add
//! that was still in flight when the pod went away (see `resume`).

mod activity;
mod add;
mod aria2;
mod auth;
mod config;
mod error;
mod filter;
mod library;
mod notify;
mod poll;
mod resume;
mod state;
mod web;

use std::sync::Arc;

use crate::config::Config;
use crate::state::AppState;

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

    let preexisting = state::Preexisting::load(config.state_dir.as_deref()).await;
    let state = AppState {
        aria2: aria2::Aria2::new(config.aria2_rpc_url.clone(), http.clone()),
        config: Arc::new(config),
        sessions: auth::session::Sessions::new(),
        activity: activity::Activity::new(),
        poll: poll::Poll::new(),
        clearing: state::Clearing::default(),
        adding: state::Adding::default(),
        preexisting,
        http,
    };

    // The poller before the watcher: the watcher reads its snapshots and would
    // otherwise sit on an empty channel until the first tick anyway. `resume`
    // reads the same channel, and only its first snapshot.
    poll::spawn(state.clone());
    notify::spawn_watcher(state.clone());
    resume::spawn(state.clone());

    let app = web::router(state.clone());

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
