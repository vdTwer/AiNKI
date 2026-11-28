# Card types

A card type draws a card its own way: a chess board, a map, a keyboard. You
bring it as a .zip and upload it under **Settings → Card types**. Each card
you write with it keeps that version, so uploading a new one never changes
old cards. A version no card uses can be deleted.

`packages/chess/` is a complete example; the Card types page offers it as a
download.

## What goes in the .zip

- `card-type.json`, the manifest.
- One ES module, the code that draws the card.
- Whatever else it loads: `.js`, `.css`, `.json`, `.png` and `.woff2` files,
  64 at most, 2 MB in all. Nothing else is accepted: no HTML, no SVG.

Zipping the folder works too; its top folder is dropped.

```json
{
  "name": "Chess",
  "description": "A position, and the move to find.",
  "module": "chess.js",
  "styles": ["board.css"],
  "fields": [
    { "key": "fen", "label": "Position (FEN)", "hint": "Shown under the field." },
    { "key": "moves", "label": "Answer" },
    { "key": "note", "label": "Why", "kind": "text", "optional": true }
  ],
  "summary": { "front": "Chess: {fen}", "back": "{moves}\n\n{note}" }
}
```

- `fields` are what the card form asks for: `kind` is `line` (the default)
  or `text` (several lines). Keys are lowercase letters, digits and `_`.
- `summary` writes the card's plain front and back from its fields, with
  `{key}` for a field. Search, card lists, exports and agents see this text.
- `styles` are loaded before the module. Unknown keys are refused, so typos
  show up at upload.

## The module

```js
export function challenge(fields, el, respond) {
  // Draw the question into el. When the learner has answered, call
  // respond(value) with what they did (any JSON value, 4 KB at most):
  // the card turns over and the value is kept with the review.
}

export function solution(fields, response, el) {
  // Draw the answer into el. response is what challenge handed over, or
  // null when the learner turned the card over themselves.
}
```

`fields` is an object of strings; empty optional fields are left out. The
learner always rates the card; a type never grades. Both functions may be
async.

The page's look arrives as CSS custom properties. Colors: `--ink`,
`--muted`, `--faint`, `--line` (hairlines), `--edge` (field borders),
`--track`, `--bg`, `--accent` (memory blue), `--fill` and `--on-fill` (the
one filled button), `--soft`, `--danger`. Fonts: `--sans` (Geist, the
frame's default), `--mono` (Geist Mono, for numbers and small caps labels)
and `--serif` (Newsreader, for card text). And `--r`, the corner radius. Use
them, and the card looks like the rest of AiNKI in light and dark mode. The
frame grows with its content.

## The sandbox

A type's code runs only in the learner's browser, never on the server, in a
frame that is sealed off:

- The frame has an origin of its own (`sandbox="allow-scripts"`, and a
  `Content-Security-Policy: sandbox` header). It cannot read AiNKI's pages,
  cookies or storage.
- It may load files only from its own package. `connect-src 'none'` stops
  `fetch`, `XMLHttpRequest` and WebSockets; it can submit no forms, open no
  windows and navigate nowhere outside AiNKI.
- It talks to the study page only through `postMessage`, and the study page
  accepts three things from it: its height, a response (a string of at most
  4 KB, stored as JSON, never shown as HTML) and that it is ready.

On the server a package is data: it is unpacked in memory with every size
counted, its file names are checked, and its files are stored in the
database and served with `nosniff` and a sandboxing CSP. Nothing in it is
run or written to disk.

One gap is known: WebRTC is not covered by `connect-src`, so a type could
in principle send the card it draws elsewhere. Today a type only ever draws
its uploader's own cards; this matters once types can be shared.
