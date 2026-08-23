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
export const APP_VERSION = "0.2.1";

export const VERSIONS = {
  mariastew: `ghcr.io/radiosilence/mariastew:${APP_VERSION}`,
} as const;
