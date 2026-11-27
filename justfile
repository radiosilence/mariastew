# mariastew

# Install the Node packages: Tailwind, the bundler and the linters.
node:
    aube install --frozen-lockfile

# Point styles/kit at koan-kit-web's source, the version Cargo.lock names, so the stylesheet is compiled from the same kit
# release as the markup its components write. The link is gitignored.
kit:
    #!/usr/bin/env bash
    set -euo pipefail
    manifest=$(cargo metadata --format-version 1 --locked | python3 -c 'import json, sys; print(next(p["manifest_path"] for p in json.load(sys.stdin)["packages"] if p["name"] == "koan-kit-web"))')
    ln -sfn "$(dirname "$manifest")" styles/kit

# Check the markup against the kōan web kit and compile styles/ into assets/app.css. The output is committed because the
# server embeds it; CI fails a build where it is stale, or where a template or the Rust that writes markup uses a class
# outside the kit.
css: node kit
    node styles/kit/kit-check.mjs templates src
    ./node_modules/.bin/tailwindcss --input styles/app.css --output assets/app.css --minify

# Write CHANGELOG.md from the fragments in changelog.d/. `--stdout` prints it instead.
changelog *args:
    python3 scripts/changelog.py {{args}}

# Open the release PR: `just release patch|minor|major`, or an exact version.
# Merging it publishes the image, the Pulumi package and the GitHub release; tags are never pushed by hand.
release version *args:
    scripts/release.sh {{version}} {{args}}
