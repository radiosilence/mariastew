//! The page, whole and in place, its stream, and every region the add sheet
//! reads or posts for, drawn through the real router with markup-shaped text in
//! every field a person, a torrent or aria2 controls: a torrent's name, its
//! file paths, aria2's error message, the activity log, a root's name, a folder
//! on disk and a folder a person makes. Each answer must carry that text
//! escaped exactly once, and no markup of the templates' own may come out
//! escaped: that is a fragment spliced in as text, which shows a page of raw
//! HTML. A new page, region or field a person controls belongs in here.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use askama_datastar::escaping::{Guard, escaped, get};
use axum::body::Body;
use axum::extract::Request;
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::routing::post;
use axum::{Json, Router};
use tower::ServiceExt;

use crate::auth::cookie::SESSION_COOKIE;
use crate::config::{Config, Root};
use crate::state::AppState;

/// Text shaped like markup, with every character escaping changes.
const HOSTILE: &str = r#"<em>hostile "q" & 'x'</em>"#;

/// The hostile text as a file system allows a name: no `/`.
fn on_disk(text: &str) -> String {
    text.replace('/', "-")
}

fn file(dir: &str, name: &str, completed: &str, selected: &str) -> serde_json::Value {
    serde_json::json!({
        "index": "1",
        "path": format!("{dir}/{name}"),
        "length": "100",
        "completedLength": completed,
        "selected": selected,
    })
}

/// A download aria2 lists, named and filed with the hostile text.
fn download(gid: &str, status: &str, dir: &str) -> serde_json::Value {
    serde_json::json!({
        "gid": gid,
        "status": status,
        "totalLength": "100",
        "completedLength": "50",
        "downloadSpeed": "10",
        "uploadSpeed": "0",
        "connections": "1",
        "numSeeders": "1",
        "dir": dir,
        "errorMessage": if status == "error" { HOSTILE } else { "" },
        "errorCode": if status == "error" { "9" } else { "0" },
        "infoHash": "d1e2f3",
        "bittorrent": {"info": {"name": HOSTILE}},
        "files": [
            file(dir, &on_disk(HOSTILE), "50", "true"),
            file(dir, &format!("{}.nfo", on_disk(HOSTILE)), "0", "false"),
        ],
    })
}

/// An aria2 holding one download moving and one failed, both hostile, which
/// refuses every magnet as one it already has.
async fn aria2(dir: String) -> String {
    let rpc = move |Json(body): Json<serde_json::Value>| {
        let dir = dir.clone();
        async move {
            let answer = |method: &str| match method {
                "aria2.tellActive" => serde_json::json!([download("a1", "active", &dir)]),
                "aria2.tellStopped" => serde_json::json!([download("b2", "error", &dir)]),
                _ => serde_json::json!([]),
            };
            let result = match body["method"].as_str().unwrap_or_default() {
                "system.multicall" => serde_json::Value::Array(
                    body["params"][0]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|call| {
                            serde_json::json!([answer(call["methodName"].as_str().unwrap())])
                        })
                        .collect(),
                ),
                "aria2.tellStatus" => download("a1", "active", &dir),
                "aria2.addUri" => {
                    return Json(serde_json::json!({
                        "jsonrpc": "2.0", "id": "mariastew",
                        "error": {"code": 1, "message": "InfoHash abc is already registered."},
                    }));
                }
                _ => serde_json::json!("OK"),
            };
            Json(serde_json::json!({"jsonrpc": "2.0", "id": "mariastew", "result": result}))
        }
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/", post(rpc)))
            .await
            .unwrap()
    });
    url
}

/// A form POST as Datastar sends one.
async fn post_form(router: &Router, url: &str, form: &[(&str, &str)]) -> (StatusCode, String) {
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(form)
        .finish();
    let response = router
        .clone()
        .oneshot(
            axum::http::Request::post(url)
                .header("datastar-request", "true")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn markup_from_people_torrents_and_aria2_is_escaped_once_everywhere() {
    let root: PathBuf =
        std::env::temp_dir().join(format!("mariastew-escaping-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(on_disk(HOSTILE))).unwrap();
    let root = root.canonicalize().unwrap();
    let dir = root.to_string_lossy().into_owned();

    let mut state = AppState::fixture(aria2(dir.clone()).await);
    state.config = Arc::new(Config {
        aria2_rpc_url: state.config.aria2_rpc_url.clone(),
        roots: vec![Root {
            name: HOSTILE.to_string(),
            path: root.clone(),
        }],
        ..Config::fixture()
    });
    state.activity.record("a1", HOSTILE);
    state.activity.record("b2", HOSTILE);
    crate::poll::spawn(state.clone());
    let cookie = format!(
        "{SESSION_COOKIE}={}",
        state
            .sessions
            .create("tester", Duration::from_secs(60))
            .await
    );
    let router = super::router(state).layer(axum::middleware::from_fn(
        move |mut request: Request, next: Next| {
            let cookie = cookie.clone();
            async move {
                request
                    .headers_mut()
                    .insert(header::COOKIE, cookie.parse().unwrap());
                next.run(request).await
            }
        },
    ));

    let guard = Guard::new(&[HOSTILE])
        .templates(concat!(env!("CARGO_MANIFEST_DIR"), "/templates"))
        .unwrap()
        // The kit's components write these.
        .elements([
            "a", "button", "svg", "path", "input", "dialog", "details", "summary", "progress",
            "dl", "dt", "dd", "header", "footer", "section", "h2", "strong",
        ]);
    let enc = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
    let signals = serde_json::json!({ "url": "/" }).to_string();
    let stream = format!("/stream/downloads?datastar={}", enc(&signals));
    let folder = format!("/browse?path={}", enc(&dir));

    let mut found = Vec::new();
    let mut check = |what: &str, status: StatusCode, text: &str, shows: bool| {
        if !status.is_success() {
            found.push(format!("{what}: {status}"));
        }
        found.extend(guard.problems(text).iter().map(|p| format!("{what}: {p}")));
        if shows && !text.contains(&escaped(HOSTILE)) {
            found.push(format!("{what}: the hostile text is missing"));
        }
    };
    for in_place in [false, true] {
        let (status, text) = get(&router, "/", in_place, Duration::from_secs(5)).await;
        check(&format!("/ (in place: {in_place})"), status, &text, true);
    }
    let (status, text) = get(&router, &stream, true, Duration::from_secs(2)).await;
    check("the stream", status, &text, true);
    for (url, shows) in [("/add", true), (&folder[..], true), ("/browse", true)] {
        let (status, text) = get(&router, url, true, Duration::from_secs(5)).await;
        check(url, status, &text, shows);
    }
    let made = on_disk(&format!("made {HOSTILE}"));
    let (status, text) = post_form(&router, "/mkdir", &[("parent", &dir), ("name", &made)]).await;
    check("/mkdir", status, &text, true);
    let (status, text) = post_form(
        &router,
        "/add",
        &[
            ("magnet", HOSTILE),
            ("magnet", "magnet:?xt=urn:btih:abc"),
            ("dir", &dir),
        ],
    )
    .await;
    check("/add, refused", status, &text, false);
    let (status, text) = post_form(
        &router,
        "/add",
        &[
            ("magnet", "magnet:?xt=urn:btih:abc"),
            ("magnet", &format!("magnet:?xt=urn:btih:def&dn={HOSTILE}")),
            ("dir", &dir),
        ],
    )
    .await;
    check("/add, partly taken", status, &text, true);

    let _ = std::fs::remove_dir_all(Path::new(&root));
    assert!(found.is_empty(), "{}", found.join("\n"));
}
