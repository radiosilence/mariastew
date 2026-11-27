# Architecture

mariastew is one binary in front of aria2: a page for adding magnets and
watching them download, the tasks that carry an add through to a narrowed
download, and a notifier. aria2 holds all the state; nothing is persisted on
this side. This document describes how the service is put together, and the
web UI in detail, which is built the way radiosilence's other server-rendered
apps are ([slsk-mcp's ARCHITECTURE.md](https://github.com/radiosilence/slsk-mcp/blob/main/ARCHITECTURE.md)
is the model). Why the service behaves as it does (the polling rates, what a
row's status means, why cancel has two meanings) is in
[docs/design.md](docs/design.md).

## The service

`main.rs` builds one `AppState` (configuration, the aria2 client, sessions,
the poller, the activity log and the in-flight marks) and serves
`web::router` on one listener.

- **Configuration** is read once at start (`Config::from_env`); a missing or
  malformed value fails startup.
- **Auth** is a session layer over the whole protected router
  (`auth::extract::require_session`), so a route added later is protected by
  construction. Sign-in is OIDC against the estate's issuer (`auth::oidc`).
- **The poller** (`poll`) is the only thing that asks aria2 what it holds: one
  task, at the page's rate while any page is open and the notifier's while
  none is, publishing snapshots on a `watch` channel.
- **Adding** (`add`) starts a magnet in aria2 and finishes it detached: it
  follows the magnet to the download it becomes, narrows that to the files
  worth keeping (`filter`) and releases it. `resume` finishes an add a restart
  interrupted.
- **Deletion** (`library`) is the only thing that deletes from the media
  library: cancelling an unfinished download (`library::remove`) and sweeping
  the garbage a finished one wrote (`library::sweep`). Both ask
  `library::written` which files a download wrote, from the record of what was
  there first (`state::Preexisting`, kept in `STATE_DIR` across restarts), and
  every deletion goes through `library::remove_contained`.
- **The notifier** (`notify`) reads the poller's snapshots, records health
  changes in the activity log, has `library` sweep the garbage a finished
  download wrote, and sends Telegram messages, drawn from `templates/notify/`.

None of these import anything from `web`.

## The web UI

Server-rendered HTML, patched over server-sent events by
[Datastar](https://data-star.dev). Markup comes from askama templates and the
components of [koan-kit-web](https://github.com/radiosilence/koan/tree/main/koan-kit-web);
the glue between them is [askama_datastar](https://github.com/radiosilence/askama-datastar).

```
src/web.rs                                 the route table, Draw, and answer helpers
src/web/assets.rs                          the embedded files and their fingerprinted URLs
src/web/layout.rs                          the document every page is drawn in
src/web/page.rs                            serve: a page whole or in place
src/web/page/downloads.rs                  the Downloads page: the list, a download, its actions
src/web/page/downloads/add.rs              the add sheet and POST /add
src/web/page/downloads/destination.rs      the destination picker, GET /browse and POST /mkdir
src/web/stream.rs                          serve: a stream paced, bounded and signed in
src/web/stream/downloads.rs                what the Downloads page's stream sends
src/web/escaping.rs                        the escaping guard (tests)
templates/layout.html and layout/          the document and its main region
templates/downloads/page.html              the page's body
templates/downloads/<region>.html          the regions it sends again
templates/notify/                          Telegram messages, outside the web UI
```

The page, its stream and its templates carry the name the UI shows:
Downloads is `page/downloads.rs`, `stream/downloads.rs` and
`templates/downloads/`. The add sheet and the destination picker are parts of
that page, so they are its submodules (`page/downloads/add.rs`) with templates
beside its own (`templates/downloads/add.html`). With one page and one stream
there is no `Page` or `Live` trait as slsk-mcp has; a second page would bring
them.

### The route table

`web::router` declares every route in one place, in three groups:

- **Pages**: `GET /` to `page::serve`.
- **Streams**: `GET /stream/downloads` to `stream::serve`.
- **Actions**: everything the page posts to or reads in place, each a handler
  in its part's module: `/add`, `/browse`, `/mkdir`, and
  `/downloads/{gid}/pause|resume|remove`.

All of it sits behind `require_datastar_on_post` (a POST Datastar did not
send is refused, which stops cross-site forgery) and the session layer.
Sign-in, `/healthz` and the assets are outside both.

### The page

`page::serve` draws the page two ways. A browser asking for the address gets
the document around the body. The page itself asking in place gets the body
morphed into `main` and the address as `$url` (`askama_datastar::address`);
the document's `askama_datastar::history("url")` keeps the address bar on it.

The page starts with the signals every page declares: `url`, which the
stream's `reopen_on` names, and `seen`, what its stream carries on from. The
add sheet adds `_added`, which only the page reads.

### The stream

The page opens `/stream/downloads` through `askama_datastar::reopen_on`, which
opens it again when it ends or goes quiet, and afresh when `$url` changes, so
a stream never follows navigation. **Every signal `reopen_on` names is
declared on every page**: an undeclared one stops the stream reopening, with
no console message.

The stream starts no work. It reads the poller's snapshots, and holding a
`poll::Viewer` is what keeps the poller at the page's rate. Snapshots are
paced by `askama_datastar::paced`, at most four redraws a second, and the
stream ends after five minutes for the page to open another. It rechecks its
session every second, so a sign-out elsewhere ends it.

A stream draws only what changed after the page did. The page's body records
each row it drew and a fingerprint of its markup in `seen`; the stream's first
pass sends only rows whose fingerprint moved. Every pass that sends a region
sets `seen` in the same answer, so the page's `seen` always matches what it
shows however the stream ends (Datastar drops a stream when the page is hidden
or the network fails), and the next carries on from there. A row is compared
against what went down this connection, not against the previous snapshot, so
a row that changed in a pass a slow client skipped still arrives. The list is
sent whole only when a download arrives, leaves or moves, since a morph by id
can neither add an element nor remove one. The kit names each request
button's busy signal afresh on every render, so fingerprints leave those names
out.

### Actions

An action changes something and answers with as little as it can:

- **Pause, resume, clear**: the stream draws the change, so the answer is
  empty, or a flash when aria2 refused. Clear waits for aria2 to let go
  (`library::remove`), so the button's own wait is the wait, and closes the
  page's question with `kit::Confirm::close` when cancelling an unfinished
  download asked one.
- **Add**: a batch that went in whole sets `_added`, which closes the sheet;
  the stream draws the new rows behind it. A refusal answers with the regions
  it changed: `#add-error` saying why, and `#magnets` holding only the magnets
  that did not go in. Closing the sheet, however it is closed, asks for it
  fresh (`GET /add`).
- **Browse, make a folder**: nothing streams the picker, so each answers with
  `#destination` as it now stands, or `#add-error` on a refusal. A flash would
  sit under the open sheet.

Form bodies whose fields repeat (the magnet rows) are read with
`axum_extra`'s `Form`. A button's download is named in its URL's path.

### Templates and markup

Every view is a struct deriving `Template` with a file under `templates/`;
there are no inline template sources. A view embeds another as
`askama_datastar::Html` (rendered with `Draw::draw`), which askama writes as
markup, and escapes every other value. Nothing builds markup with `format!`,
nothing calls `Html::trusted`, and no template uses `|safe`. A wrapper is a kit
component with a slot (`{% filter kit::wrap(…) %}`).

Datastar expressions are constants. A value that differs per row goes in a
`data-*` attribute, escaped, and the expression reads `el.dataset`: a log
line's time is `data-at`, and every button's URL is the kit's `data-url`.
Signals are named by the program, never built from data. Datastar sends every
signal except those whose names start with `_`, so a signal the server reads
(`url`, `seen`) has no underscore, and one only the page reads (`_added`, the
kit's busy signals) has one. The little the page does without the server (the
magnet rows' paste, clone and remove, and the keyboard inset) is `web/ms.ts`,
called from constant expressions.

### Patch targets

Datastar morphs a region by the id of its top-level element, so ids exist only
where the server sends a region again, declared once in its template:

| Where | Ids |
|---|---|
| Layout | `main` |
| Downloads | `downloads`, each download's `dl-<gid>`, and its `files-<gid>` and `log-<gid>`, which keep their panels open across a morph |
| The add sheet | `magnets`, `add-error`, `destination` |

Two more ids are named by something outside them rather than patched: the
sheet's `add`, which the header's button opens through `commandfor`, and the
forms `add-form` and `mkdir-form`, which the footer's add button and the
picker's folder controls belong to (`kit::Button::sends`, an input's `form=`).
The kit's own (`kit-confirm`, `flash`) are the kit's.

### The kit

Every control, status, meter, notice, flash, empty state, disclosure, sheet
and question is a koan-kit-web component; templates add layout utilities
only. Buttons that send requests are busy for exactly as long as their own
request runs; a download's buttons share a `busy_key` of its gid, so pressing
one disables the other and its status turns while either runs, and the
picker's controls share one key, so a second press while a folder is read does
nothing. The stylesheet is compiled from the crate's own source (`just kit`
links it), so one version in `Cargo.lock` names both the markup and the rules
that draw it. What the kit cannot yet express is tracked on
radiosilence/koan#2365.

### The guard

`web::escaping` builds the real router against a mock aria2, fills every field
a person, a torrent or aria2 controls with markup-shaped text (a torrent's
name and file paths, aria2's error message, the activity log, a root's name, a
folder on disk and one a person makes), and requests the page whole and in
place, the stream, the sheet and the picker, and posts the add and make-folder
forms. `askama_datastar::escaping::Guard`, told the hostile text, the templates
directory and the element names the kit writes, finds markup of the templates'
own escaped (a fragment spliced in as text), the text escaped twice, or the
text left as markup. A new page, region or field a person controls belongs in
it.
