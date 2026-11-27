declare global {
  var ms: {
    magnetRowClick(evt: MouseEvent): void;
    pasteMany(evt: ClipboardEvent): void;
    addMagnetRow(rows: HTMLElement | null): void;
  };
}

/** The magnet links in some text, when there is more than one of them. */
function severalMagnets(text: string) {
  const magnets = text.split(/\s+/).filter((t) => t.startsWith("magnet:?"));
  return magnets.length > 1 ? magnets : null;
}

/** The row an event happened in, if it happened in one. */
function rowOf(target: EventTarget | null) {
  return target instanceof Element ? target.closest("[data-magnet-row]") : null;
}

/** A row's field. */
function inputOf(row: Element) {
  const input = row.querySelector("input");
  return input instanceof HTMLInputElement ? input : null;
}

/** A blank row after `row`, cloned from it so every row is the same markup. */
function rowAfter(row: Element) {
  const next = row.cloneNode(true);
  if (!(next instanceof Element)) return row;
  const input = inputOf(next);
  if (input) input.value = "";
  row.after(next);
  return next;
}

/**
 * Fill `row` with whatever was pasted, and the rows after it too when the
 * text holds several magnets: a list copied from somewhere else goes in as
 * one row each rather than run together in one field, which an `<input>`
 * would do by dropping the line breaks.
 */
function fillFrom(row: Element, text: string) {
  const [first = "", ...rest] = severalMagnets(text) ?? [text.trim()];
  const input = inputOf(row);
  if (input) input.value = first;
  let last = row;
  for (const magnet of rest) {
    last = rowAfter(last);
    const next = inputOf(last);
    if (next) next.value = magnet;
  }
}

/**
 * Put the clipboard into a row, from that row's own Paste button.
 *
 * `readText()` is gated on transient user activation everywhere and, on iOS,
 * on a Paste bubble Safari draws itself and no page can suppress. Safari
 * anchors its bubble to the touch that asked, so asking from a button beside
 * the field puts the bubble against the field it fills.
 *
 * Whatever is on the clipboard goes in, magnet or not: a button that answers a
 * tap by doing nothing reads as broken, where a wrong value is visible and
 * `/add` says what is wrong with it. A refused prompt and a browser with no
 * clipboard API are both no-ops: either way there is a field to type into.
 */
function pasteInto(row: Element) {
  navigator.clipboard?.readText().then(
    (text) => fillFrom(row, text),
    () => {},
  );
}

/**
 * The behaviours that do not belong in an attribute: the magnet rows, which
 * the page's script adds and fills without asking the server.
 *
 * Datastar evaluates `data-on:*` as a function body, so an attribute holds
 * anything JavaScript can express, invisible to oxlint, oxfmt and `tsc`.
 * Anything with control flow in it lives here, and the attribute is a constant
 * call. A global rather than an export, because a Datastar expression sees only
 * globals, `el`, `evt` and the signals.
 */
globalThis.ms = {
  /**
   * Paste into, or remove, the row whose button was pressed. Delegated from
   * the list of rows, so a row cloned in later needs nothing wired up.
   */
  magnetRowClick(evt) {
    const button = evt.target instanceof Element ? evt.target.closest("[data-action]") : null;
    const row = rowOf(button);
    if (!(button instanceof HTMLElement) || !row) return;
    if (button.dataset.action === "remove") row.remove();
    else pasteInto(row);
  },

  /**
   * A paste of several magnets into one field spreads them over rows, the
   * same as the Paste button does. Anything else is left to the browser.
   */
  pasteMany(evt) {
    const row = rowOf(evt.target);
    const text = evt.clipboardData?.getData("text") ?? "";
    if (!row || !severalMagnets(text)) return;
    evt.preventDefault();
    fillFrom(row, text);
  },

  addMagnetRow(rows) {
    const last = rows?.lastElementChild;
    if (last) inputOf(rowAfter(last))?.focus();
  },
};

/**
 * Publish how much of the viewport the on-screen keyboard is covering, as
 * `--keyboard-inset` on the root element.
 *
 * iOS Safari shrinks the visual viewport for its keyboard and leaves the
 * layout viewport alone. Fixed positioning resolves against the layout one, so
 * the bottom sheet opened underneath the keyboard and Safari scrolled the page
 * chasing the focused field. Nothing in CSS exposes the gap between the
 * two viewports; this is the only place it can be measured. A browser that
 * resizes its layout viewport for its keyboard measures zero, which is also
 * the stylesheet's fallback.
 */
const viewport = globalThis.visualViewport;
if (viewport) {
  // visualViewport scrolls fire on every pan, and most report the same number;
  // skipping those keeps a scroll from invalidating style.
  let published = -1;
  const publish = () => {
    const covered = Math.round(
      Math.max(0, document.documentElement.clientHeight - viewport.height - viewport.offsetTop),
    );
    if (covered === published) return;
    published = covered;
    document.documentElement.style.setProperty("--keyboard-inset", `${covered}px`);
  };

  viewport.addEventListener("resize", publish);
  viewport.addEventListener("scroll", publish);
  publish();
}
