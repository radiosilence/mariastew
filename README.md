# mariastew

Paste a magnet on a phone, and later open Infuse and watch the thing.

A small Rust service in front of aria2. aria2 does the downloading and holds
all the state — there is no database on this side, so a restart re-reads
reality instead of reconciling with it, and a signed-in session lives only in
memory, so a deploy signs everyone out. The frontend is server-rendered HTML
patched over SSE ([Datastar](https://data-star.dev), vendored at
`assets/datastar.js`), not a client-side app.

Files land straight in the media library under a name you pick, so nothing has
to be moved afterwards. Why it behaves as it does — the polling rates, what a
row's status actually means, why cancel has two meanings — is in
[docs/design.md](docs/design.md).

## Deploying

Kubernetes, via the Pulumi component this repo publishes:

```ts
import { createMariastew } from "@radiosilence/mariastew-pulumi";

createMariastew(provider, namespace, {
  roots: [{ name: "tv", hostPath: "/mnt/media/tv" }],
}, {
  hostname: "dl.example.com",
  nodeLabel: "example.com/file-node",
  oidcClientSecret: secret,
  oidc: { issuer: "https://auth.example.com", clientId: "mariastew" },
});
```

See [deploy/pulumi/README.md](deploy/pulumi/README.md). Anything else — plain
docker, compose — needs the image and the environment below;
`docker-compose.yml` is a working example.

Single replica, `Recreate` strategy: the pod holds `hostPath` mounts on one
node, and a rolling surge would put the incoming pod on the same directories as
the one it is replacing.

## What it needs

Read once at boot (`src/config.rs`); a missing or malformed value fails startup
rather than the request that first needs it.

| Variable | Required | Default | What |
|---|---|---|---|
| `ROOTS` | Yes | — | `name:/path,name:/path` — each is both a mount and a root the picker may browse into or write under. The name is what the picker calls the place: a destination reads `tv/some-show`, never the mount above it |
| `PUBLIC_URL` | Yes | — | Where this is reached from outside; must match the OIDC redirect URI, since the pod cannot infer it from a request it has not had yet |
| `OIDC_ISSUER` | Yes | — | The issuer URL |
| `OIDC_CLIENT_ID` | Yes | — | |
| `OIDC_CLIENT_SECRET` | Yes | — | |
| `BIND_ADDR` | No | `0.0.0.0:8080` | |
| `ARIA2_RPC_URL` | No | `http://127.0.0.1:6800/jsonrpc` | |
| `POLL_MS` | No | `100` | How often aria2 is sampled while a page is open |
| `IDLE_POLL_MS` | No | `5000` | The same, with nobody watching |
| `TELEGRAM_BOT_TOKEN` | No | — | Must be set with `TELEGRAM_CHAT_ID` or startup fails — a token with no chat id would otherwise fail on the first send, hours after the deploy that introduced it |
| `TELEGRAM_CHAT_ID` | No | — | Both absent means no notifications, treated as normal |

It also needs **aria2** reachable at `ARIA2_RPC_URL`. The published image
contains both, run as two containers sharing a network namespace, so the RPC
never leaves loopback and needs no credential of its own.

Telegram fires only on a download finishing or failing. Starting one is not
announced — that reports something the caller just did themselves.

## Auth

An OIDC client of the estate's own Hydra instance (`src/auth/`), which is
already public with a GitHub allowlist in front of it — who may sign in is
decided there, so there is no second allowlist here. PKCE is used even though
this client also holds a secret; it costs one hash and removes the class of
attack where a leaked authorization code alone is enough.

`auth::extract::require_session` wraps the entire protected router as one
layer (`src/routes.rs`'s `router()`, mounted under it in `src/main.rs`)
rather than being applied per route. aria2's RPC has no auth of its own, so
any route that reached it without going through this layer would be full
control of the download queue and write access to the media tree — wrapping
the whole router means a route added later is protected by construction
rather than by remembering to decorate it.

## Routes

Everything below `/` requires a session; `/healthz`, the three `/assets/*`
files, and `/auth/*` are the only routes outside that layer (`src/main.rs`).

| Route | Method | What |
|---|---|---|
| `/` | GET | The page: current roots and the download list |
| `/stream` | GET | SSE stream, patching rows in place at the poll rate. see [docs/design.md](docs/design.md) |
| `/add` | POST | `magnet` + `dir` form fields — validates and starts the magnet resolving, then returns `202` immediately; the poll, the filter, and applying the selection run detached (see `routes::finish_add`), and their outcome shows up through `/stream` like any other download |
| `/downloads/{gid}/pause` | POST | |
| `/downloads/{gid}/resume` | POST | |
| `/downloads/{gid}/remove` | POST | Marks the row `clearing` and returns `204` immediately; the removal itself runs detached (`routes::clear`) and its outcome shows up through `/stream`. see [docs/design.md](docs/design.md) |
| `/browse` | GET | `path` query param — lists subdirectories of a configured root, for the destination picker |
| `/mkdir` | POST | `parent` + `name` form fields — makes a directory and returns the picker rooted there |
| `/healthz` | GET | Kubelet probe, no auth |
| `/auth/login`, `/auth/callback`, `/auth/logout` | GET | The OIDC round trip |

Every path a caller supplies — the destination on add, the path to browse or
create — is resolved through `Config::resolve`/`resolve_existing`
(`src/config.rs`), which refuses anything not lexically inside a configured
root and, for anything that must already exist, re-checks the canonicalised
result against the canonicalised roots so a symlink cannot point the request
outside them.

## Local development

```bash
mise run dev        # mariastew + aria2 against a debug build (needs docker)
mise run dev:mock   # against the mock aria2 — every download state at once
mise run seed       # populate the local tv/movies roots with realistic names
```

The stylesheet, browser script and icons are generated and committed, so
`cargo build` needs no node toolchain. Rebuilding them after changing a
template is [docs/frontend.md](docs/frontend.md).

## Releasing

Bump `version` in `Cargo.toml`, `deploy/pulumi/package.json` and
`APP_VERSION` — CI refuses a release where the three disagree. It then builds
the image, publishes the chart, and cuts the tag, in that order, so a release
means every artefact exists.

The crate compiles on the runner rather than inside the `Dockerfile`, and the
runtime base is `alpine` rather than `scratch`; both have reasons worth reading
before changing them, in [docs/design.md](docs/design.md).
