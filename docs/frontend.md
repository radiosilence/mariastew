# The frontend, and its committed assets

The stylesheet, the browser script and the icons are generated and **committed**,
so `cargo build` needs no node toolchain and the container build has one
artefact rather than two. That only holds while the committed copies match
their sources, which CI checks.

## Where the JavaScript lives

Datastar evaluates a `data-on:*` attribute as a function body, so an attribute
will hold anything JavaScript can express — which is how a click handler becomes
a program written in HTML. Signal writes and one `@action` stay in the
attribute, where they read as markup; anything with control flow in it is a
helper on `ms` in `web/app.ts`, and the attribute calls it. Elements are
reached through `data-ref` rather than `document.getElementById`, so a template
holds one definition of what an id is.

That boundary is a lint and typecheck boundary, not a taste one. `web/app.ts`
is source; `assets/app.js` is its compiled, committed output (see "Rebuilding
the browser script" below) and, like the stylesheet and the vendored bundle, is
a build artefact oxlint, oxfmt and `tsc` all ignore by name. JavaScript written
inside an attribute is seen by nothing at all until a browser runs it.

Nothing checks the two halves against each other at build time, so a test does:
`every_ms_helper_a_template_calls_is_defined` renders the page, collects every
`ms.*` it names, and fails if `app.js` does not define one. A rename is
otherwise silent until someone presses the button.

## Adding one

The magnet field has a Paste button, and that button is the only thing that
reads the clipboard. `readText()` is gated on a user gesture everywhere and,
on iOS, on a Paste bubble Safari draws itself which no page can suppress — so
the question is not whether the prompt appears but where. Safari anchors it to
the touch that asked. Reading from the header's Add tap put that bubble at the
top of the screen for a field at the bottom of it, on every open, whether or
not there was anything anyone wanted pasted; asking from a button beside the
field puts it against the field it fills, and only when asked.

Whatever is on the clipboard goes in, magnet or not. Filtering to magnets was
right while the read was automatic, since nothing unasked-for should land in a
field — but a button that answers a tap by doing nothing reads as broken,
where a wrong value is visible and `/add` says what is wrong with it. A
refused prompt and a browser with no clipboard API are both no-ops: either way
there is a field to type into.

The field is not autofocused. A keyboard on every open costs half the sheet,
and the usual flow — paste, then Add — never types into it. Tapping the field
still opens one.

Enter adds, and Enter in the picker's name field creates the folder. Neither
happened on its own: the button that adds is pinned in the footer outside the
form, so there is no submit button in it for the browser's implicit submission
to find, and the Go key on a phone keyboard — the key right under the thumb
that has just pasted — did nothing at all.

## Where the sheet sits

`.sheet` in `styles/app.css` owns the dialog's insets, width and height cap,
because a `<dialog>` already has all four from the user agent — `width:
fit-content`, `margin: auto`, `max-width: calc(100% - 6px - 2em)` — and a
margin utility only argues with those, where an author rule wins outright. The
utilities that used to sit on the element asked for the full width, got 38px
less than it, and put every pixel of the shortfall on one side.

The keyboard is the other half. iOS Safari shrinks the *visual* viewport for
it and leaves the layout viewport alone, and fixed positioning resolves
against the layout one — so a sheet at `bottom: 0` opens underneath the
keyboard, and Safari then scrolls the page chasing the focused field. Nothing
in CSS describes that gap; `app.js` measures it against `visualViewport` and
publishes `--keyboard-inset`, which the sheet subtracts from both its bottom
inset and its height. A browser that resizes its layout viewport for its own
keyboard measures zero, which is also the fallback, so this corrects only the
browsers that need correcting.

## Icons

[Lucide](https://lucide.dev) paths, copied into `templates/icons.html` as one
Askama macro each and called inline. They replaced unicode characters, which
were never really under this service's control: an emoji comes from the
device's emoji font at its own weight, colour and baseline rather than the
text's, and `⌂` is missing from enough fonts to have arrived as a tofu box on
the one control that had nothing else in it.

Nothing is fetched at runtime — they weigh less than the request that would
fetch them, and there is no JavaScript on this page to draw them with.
The shared presentation attributes live on `.icon` in `styles/app.css`, sized
in `em` so an icon is the size of the text it sits beside; a macro takes a
class for the cases that want otherwise.

A `<summary>`'s own marker comes from the user agent, so it is a filled
triangle in Chrome and something else everywhere else — the same problem
arriving through a different door. `.disclosure` takes it off and turns the
chevron inside instead; a disclosure written later gets that by using the same
two class names.

Adding one: call `icons::<name>(class="")` from a template, run

```sh
mise run icons
```

and it adds a macro for any name a template calls that `icons.html` doesn't
define yet, sourced from `lucide-static`'s `<name>.svg` (kebab-cased) —
the `<path>`/`<polyline>`/`<circle>` children and nothing else, since
everything on the `<svg>` itself is on `.icon` already. It never touches a
macro that already exists: `trash`'s shape is actually `trash-2.svg`'s under
a plainer name, and nothing records that on purpose, so a name-guessed
overwrite would have silently dropped two of its paths. It warns instead, if
an existing macro's content no longer matches its name-guessed source — worth
a look, not necessarily wrong. Rebuild the stylesheet too if the call site
introduced a class.

Every icon names itself with `data-icon`, which is what tests assert on — a
count of `<svg>`s only says the number changed, which every new call site
does. `views::tests::nothing_is_drawn_with_a_unicode_glyph` fails the build if
a character is used instead.

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

One daisyUI theme, `mariastew`, defined in `styles/app.css`; daisyUI's own are
switched off (`themes: false`), so the stylesheet carries the one palette it
renders. It is the app icon read out loud — amber is the stew, steel is the
magnet, red is its poles — so `btn-primary`, a download bar and a failed
download are literally the three colours in `icons/icon.svg`, and `base-200` is
the icon's own tile. `success` and `info` are the two that are not in the
drawing: they have to be told apart from each other and from `warning` at a
glance across a progress bar, which is a job for hue distance rather than for
brand.

It is dark only. This is a thing you open at night to start a download, and a
second palette is a second set of contrast decisions to keep true for a
preference nobody has expressed yet.

Flat, and rounded only enough to notice — `--depth` and `--noise`, the gradient
overlay and surface texture daisyUI 5 adds over a flat theme, are both off, and
the radii step up with the size of the thing rather than making a badge a pill.
That is also why the header carries a border instead of the shadow it used to:
on a dark surface a small drop shadow draws a separation the eye cannot see.

The page background is the icon's silhouette, once, very faint, `fixed` so it
does not scroll. It is the silhouette and not the icon because flattening the
drawing to one colour fills the gap between the poles and the U stops reading;
for the same reason the poles are cut away from the legs by a gap rather than
sitting on them, since a gap says what the colour change said using the only
ink one flat colour has.
With a full list it is a background in the literal sense — the rows on top of
it are opaque, so it is seen around and between them and mostly not seen at
all. Where it does the work is everything that is not a full list: the empty
state, the sign-in page, a short queue, the space under the last row.

## Rebuilding the stylesheet

The stylesheet is generated with Tailwind + DaisyUI but the *output*
(`assets/app.css`) is committed, so `cargo build` needs no Node toolchain —
only editing a template does:

```sh
mise run css
```

This runs `tailwindcss` from `node_modules/.bin` (this repo's own
devDependencies — `package.json` declares `mariastew-assets`,
separate from the Rust crate) against `styles/app.css`, minified, into
`assets/app.css`.

## Rebuilding the browser script

Same deal as the stylesheet: `web/app.ts` is source, `assets/app.js` is the
committed output, and only editing the source needs Node:

```sh
mise run js
```

This runs `tsc` from `node_modules/.bin` against
`tsconfig.web.json` — a browser config (DOM lib, real emit), which is what
`mise run typecheck` picks up. It stands alone rather than extending anything,
which is what made moving here a directory copy.

There is no bundler. `app.ts` imports nothing, so there is no module graph to
build — and the one thing worth bundling in, Datastar, cannot come from npm at
the version this app runs (`@starfederation/datastar` publishes a
`1.0.0-beta`, against the v1.0.2 release vendored at `assets/datastar.js`), so
it stays a `<script>` tag rather than an import.
