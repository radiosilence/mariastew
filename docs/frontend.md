# The frontend, and its committed assets

The stylesheet and the icons are generated and **committed**, so `cargo build`
needs no node toolchain and the container build has one artefact rather than
two. That only holds while the committed stylesheet matches its sources, which
CI's `web` job checks by rebuilding it and failing on any difference. The same
job runs `mise run lint` (oxlint, and oxfmt in check mode, over the scripts and
the deploy chart; `mise run fmt` applies it). Node packages are installed with
aube from `aube-lock.yaml`.

## Where the JavaScript lives

Nowhere of the app's own. Datastar evaluates a `data-on:*` attribute as a
function body, so an attribute will hold anything JavaScript can express,
invisible to oxlint, oxfmt and `tsc`. Every expression in the markup is
therefore the kit's: the magnet rows are its `RepeatField`, a log line's time
is its `Time`, and its `Sheet` keeps itself above an on-screen keyboard. The
page loads Datastar, from askama_datastar, and nothing else.

## Adding one

The magnet field has a Paste button, and that button is the only thing that
reads the clipboard. `readText()` is gated on a user gesture everywhere and,
on iOS, on a Paste bubble Safari draws itself which no page can suppress — so
the question is not whether the prompt appears but where. Safari anchors it to
the touch that asked. Reading from the header's Add tap put that bubble at the
top of the screen for a field at the bottom of it, on every open, whether or
not there was anything anyone wanted pasted; asking from a button beside the
field puts it against the field it fills, and only when asked.

Whatever is on the clipboard goes in, magnet or not: a button that answers a
tap by doing nothing reads as broken, where a wrong value is visible and `/add`
says what is wrong with it. A
refused prompt and a browser with no clipboard API are both no-ops: either way
there is a field to type into.

Several magnets go in as several rows, one field each, under one destination:
the kit's `RepeatField`. A paste holding several lines — a list copied from a
forum post or a chat — fills a row per line, from either the Paste button or a
native paste, because an `<input>` would otherwise drop the line breaks and run
them together into one invalid link. It does not split on spaces: a magnet's
`dn=` copied from a page can hold a literal one, and splitting there would cut
one link into several invalid rows. Every line gets a row, magnet or not, so a
list with prose between its links leaves rows for `/add` to refuse by number;
clearing those is quicker than finding a link a filter dropped. The field is
plain text with the URL keyboard rather than `type="url"`, so the browser's
own validation never answers before `/add`'s row-numbered one.

While the sheet reads its body the form it sends is not there yet, so the kit
hides its footer actions until the body arrives, and a failed read closes the
sheet with a failure flash.

`/add` takes the batch whole for validation and one magnet at a time for
aria2: a bad row refuses everything, since nothing has been added yet and that
row is the one to fix, but a magnet already in the list does not stop the
others. The rows it did take are dropped from the sheet, so pressing Add again does
not fail every row that already succeeded.

The field is not autofocused. A keyboard on every open costs half the sheet,
and the usual flow — paste, then Add — never types into it. Tapping the field
still opens one.

Enter adds, and Enter in the picker's name field creates the folder. The
button that adds is pinned in the sheet's footer outside the form, and the
folder controls sit inside the add form while belonging to another, so each
button is made its form's submit with the kit's `Button::sends`, which is what
the browser's implicit submission presses.

When some of a batch went in and some did not, the server answers with the
magnet rows that are left, so pressing Add again sends only those. The sheet
reads its body from the server each time it opens (`kit::Sheet::loads`), so an
attempt abandoned by any of the ways a dialog closes is gone on the next
opening.

## Where the sheet sits

The kit's `.sheet` owns the dialog's insets, width and height cap,
because a `<dialog>` already has all four from the user agent — `width:
fit-content`, `margin: auto`, `max-width: calc(100% - 6px - 2em)` — and a
margin utility only argues with those, where an author rule wins outright.
Utilities on the element asking for the full width get 38px less than it, and
put every pixel of the shortfall on one side.

The keyboard is the other half. iOS Safari shrinks the *visual* viewport for
it and leaves the layout viewport alone, and fixed positioning resolves
against the layout one, so a sheet at `bottom: 0` would open underneath the
keyboard. The kit's `Sheet` measures the gap against `visualViewport` and
publishes `--keyboard-inset`, which its stylesheet subtracts from the sheet's
bottom inset and height.

## Icons

The kit's icon set (`kit::icon`), drawn inline as `<svg>` at the size of the
text beside it, so nothing is fetched and no glyph depends on the device's
fonts. An icon beside its words is hidden from a screen reader; an icon-only
button carries its label as its accessible name.

## The app icon

`icons/icon.svg` is the drawing and everything else is rendered from it by

```sh
mise run app-icons
```

which needs `brew install librsvg`. The outputs are committed, so neither the
container build nor a plain `cargo build` needs it — `main.rs` compiles them in
with `include_bytes!`, the same way it does the stylesheet.

A horseshoe magnet holding the stew between its poles, which is what the name
and the job are: aria2 said out loud is "Maria Stew", and what it does is fetch
things off strangers with magnet links. It is drawn as flat fills rather than
in the Lucide line style above, because 16px is the size that decides an icon —
it is the browser tab, and a 2-unit stroke on a 24 grid is a third of a pixel
there. Nine of the ten it was chosen from are in the history of
[jaritanet#373](https://github.com/radiosilence/jaritanet/issues/373).

Two shapes come out of the one drawing, and the difference is who rounds the
corners. Anything shown as given keeps the tile's own: the favicon, and the
manifest's `purpose: any` pair, which is what an install prompt, a task
switcher and a file browser put on screen. Only what is definitely going to be
masked renders full-bleed — the `maskable` pair, which Android cuts to
whatever the launcher's shape is, and `apple-touch-icon`, which iOS always cuts
to a squircle. A pre-rounded tile handed to a mask is rounded twice and shows
the page's background in the gap.

That is why there are two of each size rather than one declared `any
maskable`: a single full-bleed icon doing both jobs is a bare square
everywhere it is not masked. And it is why `icon.svg` keeps its tile and its
mark as separate elements, so `scripts/gen-app-icons.ts` can compose the
full-bleed variants from `#mark` rather than from a second drawing to keep in
step.

All of them, and the manifest, are served from outside the session layer along
with the stylesheet: the sign-in page needs an icon too, and a manifest is
fetched without credentials by default, so one behind the layer would answer an
install prompt with a login redirect. `/favicon.ico` is registered at the root
as well as under `/assets`, because that is the path a browser asks for on its
own before it has parsed any `<link>`.

## The theme

The look is the one radiosilence's other web pages share, and the kōan web kit
([koan-kit-web](https://github.com/radiosilence/koan/tree/main/koan-kit-web))
draws it: its components write every control the templates show, and its
stylesheet, compiled from the crate's own source, draws them. The rules behind it are kōan's
[`docs/design/koan-theme.md`](https://github.com/radiosilence/koan/blob/main/docs/design/koan-theme.md).
`styles/app.css` holds only what is particular to mariastew: the accent and the
watermark. A look the app needs and the kit
lacks is added to the kit rather than here.

The kit removes Tailwind's palette, radii, shadows, blurs and type sizes, so a
class such as `text-sm` or `rounded-lg` compiles to nothing and silently loses
its style. `just css` runs the kit's `kit-check` over `templates/`, `src/` and
`web/` first and fails on any class outside the kit; a deliberate exception is
marked `kit: raw` with its reason.
Geist Mono throughout, square corners, no shadows, blur or gradients, regions
separated by space or a hairline, and the app's own words in lower case. What
the app did not write keeps its case: torrent and file names, directory names,
aria2's error messages.

The theme gives a dark and a light palette and follows the reader's system
setting. The accent, `--brand`, is overridden after the import with the stew's
amber from `icons/icon.svg`, darkened on light to stay legible; both values and
their contrast against `--surface` are stated where they are set in
`styles/app.css`.

A download bar is the one place colour carries a status, so it is drawn in the
kit's status tones rather than the accent: `--ok` for moving or done, `--warn`
for stalled, `--bad` for failing, and `--muted` for waiting on something other
than the swarm. A row's per-file bars take the same tone as its own.
Each tone goes beside its word (`Health::word`, the tone in
`web::page::downloads`), so colour is never the only way to read a row.

The page background is the icon's silhouette, once, very faint, `fixed` so it
does not scroll. It is the silhouette and not the icon because flattening the
drawing to one colour fills the gap between the poles and the U stops reading;
for the same reason the poles are cut away from the legs by a gap rather than
sitting on them. The rows on top of it are opaque, so it shows in the empty
state, the sign-in page, a short queue and the space under the last row.

## Rebuilding the stylesheet

The output (`assets/app.css`) is committed, so `cargo build` needs no Node
toolchain; only editing a template or `styles/app.css` does:

```sh
just css
```

This installs the Node packages, links `styles/kit` to the koan-kit-web crate's
source at the version `Cargo.lock` names, runs the kit's `kit-check` over
`templates/` and `src/`, and compiles `styles/app.css` into
`assets/app.css`. The kit's typeface is served from the crate
(`koan_kit_web::font::serve`) beside the stylesheet, where it asks for it.
