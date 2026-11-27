# mariastew

A Rust service in front of aria2: paste a magnet on a phone, and the download
lands in the media library. README.md covers deployment, configuration and the
routes; docs/design.md explains why it behaves as it does.

## Web

The web UI is askama templates and koan-kit-web components, patched over SSE by
Datastar through askama_datastar. Its layout, the rules every page and stream
follows, and the table of ids are in [ARCHITECTURE.md](ARCHITECTURE.md); keep
that table current when a region gains or loses an id. A new page, region or
field a person controls goes in the escaping guard (`src/web/escaping.rs`).

## Building

`cargo build` needs no Node: the stylesheet is committed. After changing a
template or `styles/`, run `just css`. See docs/frontend.md.

## Changes

Every pull request adds a changelog fragment under `changelog.d/unreleased/`
(changelog.d/README.md); CHANGELOG.md is generated and never edited by hand.
