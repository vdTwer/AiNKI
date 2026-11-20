# Chess cards

A chess card shows a position and asks for one move. The first real move
played on the board is the answer: it turns the card, says whether it was
right and shows the comment. Arrows (right-drag) never count. A right move is
graded Good, a wrong one Again; there is no self-grading.

## How a chess card is stored

Everything lives in the card's two sides, so search, sharing, merging and
agents work without special cases. `cards.kind` is `chess`.

- Front: the position as FEN on the first line. An optional question or hint
  follows after a blank line (Markdown).
- Back: the right moves on the first line, in SAN, separated by commas
  (`Nf3, Ng5`). An optional comment follows after a blank line (Markdown).

Moves may be written in SAN (`Nf3`, `exd5`, `O-O`, `e8=Q`) or UCI (`g1f3`,
`e1g1`); they are checked against the position and saved as SAN, which is
also all people ever see. When a right move gives mate, any mating move
counts as right.

The server side (FEN, SAN, UCI, legality) uses the `cozy-chess` crate (MIT),
see `src/chess.rs`.

## Importing

The import page takes a list of positions, one per line:

- EPD, the usual format of puzzle collections: `<FEN> bm Nf3; c0 "comment";`
- A FEN followed by the moves and an optional comment:
  `<FEN> Nf3, Ng5; comment`
- Lichess's puzzle CSV (`PuzzleId,FEN,Moves,…`): the first move is played,
  the second is the answer.

## The board

`src/static/chess/` holds the vendored browser side: cm-chessboard 8.15.4
(MIT) and chess.js 1.4.0 (BSD-2-Clause), bundled into one minified file,
`board.js`, which exposes `window.AinkiChess`. GPL boards such as
chessground are not allowed: they would put AiNKI's frontend under the GPL.
`app.js` loads it only when a page has a chess card.

To rebuild it (outside the repository; npm is used only here):

```sh
npm i --ignore-scripts cm-chessboard@8.15.4 chess.js@1.4.0 esbuild@0.28.2
cat > entry.js <<'JS'
export {Chessboard, COLOR, INPUT_EVENT_TYPE, BORDER_TYPE, FEN} from "cm-chessboard/src/Chessboard.js"
export {Markers, MARKER_TYPE} from "cm-chessboard/src/extensions/markers/Markers.js"
export {Arrows, ARROW_TYPE} from "cm-chessboard/src/extensions/arrows/Arrows.js"
export {PromotionDialog, PROMOTION_DIALOG_RESULT_TYPE} from "cm-chessboard/src/extensions/promotion-dialog/PromotionDialog.js"
export {RightClickAnnotator} from "cm-chessboard/src/extensions/right-click-annotator/RightClickAnnotator.js"
export {Chess} from "chess.js"
JS
npx esbuild entry.js --bundle --format=iife --global-name=AinkiChess --target=es2018 --minify --outfile=board.min.js
```

Then put both licenses above it as the comment at the top of `board.js`, and
copy `assets/chessboard.css` plus the arrows, markers and promotion-dialog CSS
into `board.css`, and the sprites `pieces/standard.svg`,
`extensions/arrows/arrows.svg` and `extensions/markers/markers.svg`.
