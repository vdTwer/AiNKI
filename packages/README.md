# Card type packages

Card types people bring to AiNKI (see `docs/card-types.md`). Each folder is
one package; the website zips `chess/` for the download on the Card types
page, so it is built into the server.

## chess

`board.js` is cm-chessboard 8.15.4 (MIT) and chess.js 1.4.0 (BSD-2-Clause),
bundled into one ES module. To rebuild it (outside the repository; npm is
used only here):

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

Put both licenses at the top of `board.js` as a comment, then the bundle,
then `export default AinkiChess;`. `board.css` is cm-chessboard's
`assets/chessboard.css` with the arrows, markers and promotion-dialog CSS.

The frame may fetch nothing, so the board's sprites (`assets/pieces/standard.svg`,
`assets/extensions/markers/markers.svg`, `assets/extensions/arrows/arrows.svg`)
are inlined as strings in `sprites.js`.
