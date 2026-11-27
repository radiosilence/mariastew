//! The destination picker inside the add sheet, `#destination`: the folder a
//! download lands in, browsed from the configured roots, with a field to make
//! a new one.
//!
//! Nothing streams it, so browsing and making a folder each answer with the
//! picker as it now stands, and a refusal answers with the sheet's error
//! region. Every path a person sends is resolved through
//! `Config::resolve_existing`, which refuses anything outside a root and, after
//! canonicalising, anything a symlink points out of one.

use std::time::Duration;

use askama::Template;
use axum::extract::{Query, State};
use axum::response::Response;
use axum_extra::extract::Form;
use koan_kit_web as kit;

use super::add;
use crate::state::AppState;
use crate::web::Draw;

/// The picker at one folder, or at the list of roots.
#[derive(Template)]
#[template(path = "downloads/destination.html")]
pub(in crate::web) struct Destination {
    /// Absent at a root, which is where browsing upward stops.
    parent: Option<String>,
    /// The absolute path, which the hidden inputs submit and aria2 is
    /// eventually handed; never what is shown.
    path: String,
    /// The same place as the picker says it (`Config::label`), starting at the
    /// root that was picked: the only form that reaches the screen. Empty at
    /// the roots, where nothing has been picked yet.
    label: String,
    folders: Vec<Folder>,
}

/// A folder the picker offers: a root, or one under the folder shown.
struct Folder {
    name: String,
    path: String,
}

impl Destination {
    /// The roots, as the sheet opens.
    pub(in crate::web) fn roots(state: &AppState) -> Self {
        Self {
            parent: None,
            path: String::new(),
            label: String::new(),
            folders: state
                .config
                .roots
                .iter()
                .map(|r| Folder {
                    name: r.name.clone(),
                    path: r.path.to_string_lossy().into_owned(),
                })
                .collect(),
        }
    }

    /// The folder at `path`, which must exist inside a root.
    async fn at(state: &AppState, path: &str) -> Result<Self, &'static str> {
        // `resolve_existing`, not `resolve`: the folder has to exist to be
        // listed, and canonicalising it is what catches a symlink pointing
        // somewhere lexically inside a root but not really.
        let dir = state
            .config
            .resolve_existing(path)
            .await
            .ok_or("That folder is not inside the library.")?;

        let unreadable = |e: std::io::Error| {
            tracing::warn!(path = %dir.display(), error = %e, "browse: could not read a folder");
            "That folder could not be read."
        };
        let mut entries = tokio::fs::read_dir(&dir).await.map_err(unreadable)?;
        let mut folders = Vec::new();
        while let Some(entry) = entries.next_entry().await.map_err(unreadable)? {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || !entry.file_type().await.map_err(unreadable)?.is_dir() {
                continue;
            }
            folders.push(Folder {
                name,
                path: entry.path().to_string_lossy().into_owned(),
            });
        }
        folders.sort_by(|a, b| a.name.cmp(&b.name));

        // `Some` only when the parent is itself inside a root: an absent link
        // is what tells the picker that browsing upward stops here.
        let parent = match dir.parent() {
            Some(p) => {
                let p = p.to_string_lossy().into_owned();
                state.config.resolve_existing(&p).await.map(|_| p)
            }
            None => None,
        };

        // Labelled from the canonical folder rather than the path asked for:
        // the two differ only through a symlink, and the canonical one is
        // where a download lands.
        Ok(Self {
            parent,
            label: state.config.label(&dir),
            path: path.to_string(),
            folders,
        })
    }

    /// The address that shows the folder at `path`.
    fn browse(&self, path: &str) -> String {
        let query: String = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("path", path)
            .finish();
        format!("/browse?{query}")
    }
}

#[derive(serde::Deserialize)]
pub(in crate::web) struct BrowseQuery {
    #[serde(default)]
    path: String,
}

/// The picker at the folder `path` names, or at the roots for none.
pub(in crate::web) async fn browse(
    State(state): State<AppState>,
    Query(q): Query<BrowseQuery>,
) -> Response {
    shown(&state, &q.path).await
}

async fn shown(state: &AppState, path: &str) -> Response {
    let picker = if path.is_empty() {
        Ok(Destination::roots(state))
    } else {
        Destination::at(state, path).await
    };
    match picker {
        Ok(picker) => crate::web::one(&picker.draw()),
        Err(message) => crate::web::one(&add::Error::of(message).draw()),
    }
}

#[derive(serde::Deserialize)]
pub(in crate::web) struct MkdirForm {
    #[serde(default)]
    parent: String,
    #[serde(default)]
    name: String,
}

/// How long the filesystem gets to make a folder. A `mkdir` against spinning
/// disks takes a moment; this long is a mount that has gone away.
const FS_TIMEOUT: Duration = Duration::from_secs(10);

/// One folder, not a path of them.
fn is_valid_dir_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/')
}

/// Makes the folder `name` under `parent` and answers with the picker inside
/// it.
pub(in crate::web) async fn mkdir(
    State(state): State<AppState>,
    Form(form): Form<MkdirForm>,
) -> Response {
    let refuse = |message: &str| crate::web::one(&add::Error::of(message).draw());
    let name = form.name.trim();
    if !is_valid_dir_name(name) {
        return refuse("A folder name cannot be empty, `.`, `..` or hold a `/`.");
    }
    // The parent has to exist for the join to mean anything, and a symlinked
    // parent is caught here rather than trusted because its name looked right.
    let Some(parent) = state.config.resolve_existing(&form.parent).await else {
        return refuse("Choose a folder inside the library first.");
    };
    let target = parent.join(name);
    let target_str = target.to_string_lossy().into_owned();

    match tokio::time::timeout(FS_TIMEOUT, tokio::fs::create_dir_all(&target)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            tracing::warn!(path = %target_str, error = %e, "mkdir failed");
            return refuse("That folder could not be made.");
        }
        Err(_) => {
            tracing::warn!(path = %target_str, "mkdir timed out");
            return refuse("The library did not answer in time.");
        }
    }
    // `target` exists now, so it can be canonicalised, which catches a name
    // that was an existing symlink `create_dir_all` walked straight through.
    if state.config.resolve_existing(&target_str).await.is_none() {
        tracing::warn!(path = %target_str, "mkdir target escaped its root");
        return refuse("That folder is not inside the library.");
    }
    shown(&state, &target_str).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picker(parent: Option<&str>, path: &str, label: &str, folders: &[(&str, &str)]) -> String {
        Destination {
            parent: parent.map(str::to_string),
            path: path.to_string(),
            label: label.to_string(),
            folders: folders
                .iter()
                .map(|(name, path)| Folder {
                    name: name.to_string(),
                    path: path.to_string(),
                })
                .collect(),
        }
        .draw()
        .as_str()
        .to_string()
    }

    /// A folder named to close whatever it is pasted into reaches only escaped
    /// attributes and text, never an expression: the path travels in the
    /// button's `data-url`, and the expression that reads it is the kit's.
    #[test]
    fn a_folder_name_cannot_break_out_of_the_picker() {
        let html = picker(
            Some("/mnt/kontent"),
            "/mnt/kontent/tv",
            "tv",
            &[("'+alert(1)+'", "/mnt/kontent/tv/'+alert(1)+'")],
        );
        assert!(!html.contains("'+alert(1)+'"), "{html}");
        for line in html.lines().filter(|l| l.contains("data-on:")) {
            assert!(!line.contains("alert(1)"), "{line}");
        }
    }

    /// The label is shown, not the path: the mount above the root is the same
    /// for every destination, so it would spend the width the tail needs
    /// (#303). The absolute path is still what is submitted.
    #[test]
    fn the_current_folder_is_shown_from_its_root_and_submitted_whole() {
        let tail = "Static.Tide.1994.Criterion.1080p.BluRay.x265.HEVC.10bit.AAC.5.1";
        let html = picker(
            Some("/mnt/kontent/movies"),
            &format!("/mnt/kontent/movies/{tail}"),
            &format!("movies/{tail}"),
            &[],
        );
        assert!(html.contains(&format!(">movies/{tail}<")), "{html}");
        assert!(
            !html.contains(&format!(">/mnt/kontent/movies/{tail}")),
            "{html}"
        );
        assert!(
            html.contains(&format!(r#"name="dir" value="/mnt/kontent/movies/{tail}""#)),
            "{html}"
        );
    }

    /// Two folders alike but for a tail long enough to wrap: both whole names
    /// are on the page, since the tail is what tells them apart.
    #[test]
    fn folder_names_are_never_cut() {
        let common = "Harbourlight.2004.Criterion.Collection.2160p.UHD.BluRay.REMUX.HEVC";
        let (one, two) = (format!("{common}.PART1"), format!("{common}.PART2"));
        let html = picker(None, "", "", &[(&one, "/a"), (&two, "/b")]);
        assert!(html.contains(&one) && html.contains(&two));
        assert!(!html.contains("line-clamp") && !html.contains("truncate"));
    }

    #[test]
    fn an_empty_folder_and_no_roots_each_say_so() {
        assert!(picker(Some("/m"), "/m/d", "movies/d", &[]).contains("this folder is empty"));
        assert!(picker(None, "", "", &[]).contains("no destinations are configured"));
    }

    #[test]
    fn a_folder_name_is_one_folder() {
        assert!(is_valid_dir_name("Show Name S02"));
        for bad in ["", ".", "..", "a/b"] {
            assert!(!is_valid_dir_name(bad), "{bad}");
        }
    }
}
