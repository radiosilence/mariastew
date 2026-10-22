### Fixed

- An add interrupted by a restart is picked up again. 0.1.23 claimed this and did not do it: `--bt-load-saved-metadata` resolves a restored magnet off the disk before aria2 answers a single RPC, so it comes back named after its file rather than `[METADATA]…` and the rule that looked for one matched nothing. The two fixes cancelled each other out ([#362](https://github.com/radiosilence/jaritanet/issues/362))
- An unfinished add is now recognised by what aria2 holds — paused, with no `select-file` — and reconciled continuously rather than once at startup, so a magnet that resolves long after boot is picked up too. A download somebody paused on purpose already carries a selection, and is left alone ([#362](https://github.com/radiosilence/jaritanet/issues/362))
- An adopted add is narrowed in place when its metadata pass is already over, rather than waiting ten minutes for a `followedBy` that will never appear while the download runs unfiltered ([#362](https://github.com/radiosilence/jaritanet/issues/362))

### Changed

- aria2 creates the followed download stopped (`--pause-metadata`), so the filter's selection lands before any content is fetched. The add no longer pauses it — that raced with aria2's own stop, fetched the whole torrent meanwhile, and stranded the download outright if the process died in the window ([#362](https://github.com/radiosilence/jaritanet/issues/362))
