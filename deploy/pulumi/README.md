# `@radiosilence/mariastew-pulumi`

Stands mariastew up in Kubernetes: one pod, two containers built from the same
image sharing a network namespace, so aria2's RPC never leaves loopback and
needs no credential of its own.

```ts
import { createMariastew } from "@radiosilence/mariastew-pulumi";

const { routes, oidc } = createMariastew(
  provider,
  namespace,
  {
    roots: [
      { name: "tv", hostPath: "/mnt/media/tv" },
      { name: "movies", hostPath: "/mnt/media/movies" },
    ],
  },
  {
    hostname: "dl.example.com",
    nodeLabel: "example.com/file-node",
    oidcClientSecret: someSecret,
    oidc: { issuer: "https://auth.example.com", clientId: "mariastew" },
  },
);
```

## What it does not do

It has no address, no credentials and no opinion about where the media lives.
Hostnames, secrets and the roots are passed in.

It states no scheduling policy either. `limits` is a ceiling you choose;
`requests` is left to you, because how much of a ceiling to reserve depends on
what else shares the node. Omit it and Kubernetes defaults the request to the
limit.

What comes back is where it now stands and the OAuth client it needs
registered — including its redirect URI, built from the hostname it was told to
publish at, so the allowlist entry cannot name somewhere the service is not.

## Peer dependencies

`@pulumi/pulumi`, `@pulumi/kubernetes` and `zod` are peers. A Pulumi program
must have exactly one copy of each; two would be two engines, two provider
registries, and two mutually unassignable sets of Zod types.

## Versioning

The package version is the crate version. They are one project and always ship
together, so one number is honest where two would only invite them to disagree
— pinning `@radiosilence/mariastew-pulumi@0.2.0` says exactly which build you
get, and CI refuses a release where `Cargo.toml`, `package.json` and
`src/versions.ts` disagree.

## Installing

Published to GitHub Packages, which requires authentication even for public
packages. Consumers need a `read:packages` token:

```
# .npmrc
@radiosilence:registry=https://npm.pkg.github.com
//npm.pkg.github.com/:_authToken=${NODE_AUTH_TOKEN}
```

In Actions, `secrets.GITHUB_TOKEN` is enough.
