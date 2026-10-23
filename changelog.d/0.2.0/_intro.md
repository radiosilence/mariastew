### Added

- **Its own repository, and a Pulumi component published from it as
  `@radiosilence/mariastew-pulumi`.** mariastew lived in the deployment that
  ran it, which meant a change to the app and a change to how it is deployed
  were the same commit in the same tree — convenient right up until anyone else
  wanted to run it, and invisible as a coupling until then.

  The chart carries no hostname, no credentials and no media roots; those
  belong to the deployment that instantiates it and are passed in. It states no
  scheduling policy either — `limits` is a ceiling the caller chooses and
  `requests` is theirs to decide, because how much of a ceiling to reserve
  depends on what else shares the node.

  The package version is the crate version, and CI refuses a release where they
  disagree. The chart publishes only after the image it pins is in the registry,
  and the release is cut after both.

### Changed

- Release tags lose their `mariastew-` prefix: one repository releasing one
  thing needs no way to say which.
