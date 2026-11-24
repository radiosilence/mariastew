# mariastew

# Install the Node packages, the shared theme (@radiosilence/theme) among them, writing the registry token first if
# missing.
theme:
    @test -s .npmrc.auth || just auth
    aube install --frozen-lockfile

# Write the GitHub Packages token .npmrc reads: GH_TOKEN in CI, the gh login otherwise. A token in .npmrc itself would
# be an ${ENV} aube refuses to expand in a project's auth settings.
auth:
    #!/usr/bin/env bash
    set -euo pipefail
    token="${GH_TOKEN:-$(gh auth token 2>/dev/null || true)}"
    [ -n "$token" ] || { echo "no GitHub token: run 'gh auth login', or set GH_TOKEN" >&2; exit 1; }
    printf '//npm.pkg.github.com/:_authToken=%s\n' "$token" > .npmrc.auth

# Check the markup against the kōan web kit, compile styles/ into assets/app.css and copy the theme's typeface beside it,
# where the stylesheet's relative font URL points. Both outputs are committed because the server embeds them; CI fails a
# build where either is stale, or where a template or the Rust that writes markup uses a class outside the kit.
css: theme
    node node_modules/@radiosilence/theme/kit-check.mjs templates src web
    ./node_modules/.bin/tailwindcss --input styles/app.css --output assets/app.css --minify
    cp node_modules/@radiosilence/theme/geist-mono.woff2 assets/geist-mono.woff2

# Write CHANGELOG.md from the fragments in changelog.d/. `--stdout` prints it instead.
changelog *args:
    python3 scripts/changelog.py {{args}}

# Open the release PR: `just release patch|minor|major`, or an exact version.
# Merging it publishes the image, the Pulumi package and the GitHub release; tags are never pushed by hand.
release version *args:
    scripts/release.sh {{version}} {{args}}
