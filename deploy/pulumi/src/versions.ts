/**
 * The version this chart deploys.
 *
 * `APP_VERSION` is this repository's own crate version, and the package version
 * is the same number. mariastew and the chart that deploys it are one project
 * and always ship together, so one number is honest where two would only invite
 * them to disagree — pinning `@radiosilence/mariastew-pulumi@0.1.29` says
 * exactly which build you get. CI refuses a release where `Cargo.toml`,
 * `package.json` and this disagree.
 */
export const APP_VERSION = "0.10.2";

export const VERSIONS = {
  mariastew: `ghcr.io/radiosilence/mariastew:${APP_VERSION}`,
  alpine: "alpine:3.24@sha256:f59abe656b692d901e184be3445e084786602108599d2a51d229c0f5a48c71bf",
} as const;
