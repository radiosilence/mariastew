### Added

- `web/app.ts` is now typechecked and bundled with rolldown into a content-hashed `assets/app-<hash>.js`, so a deploy's new script can no longer be served from a browser cache still holding the old one ([#340](https://github.com/radiosilence/jaritanet/issues/340))
- `mise run mariastew:icons` adds a macro to `templates/icons.html` for any `icons::<name>` a template calls that isn't defined yet, sourced from the `lucide-static` package ([#357](https://github.com/radiosilence/jaritanet/issues/357))
