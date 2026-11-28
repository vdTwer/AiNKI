// Chess cards for AiNKI: a position, and the move to find. The answer may be
// a whole line (`Nf3 Nc6 Bb5`): the learner plays their moves and the board
// answers with the replies in between. Nothing is graded here; the learner
// sees the answer and rates the card.
//
// Moves may be written in SAN (Nf3, exd5, O-O, e8=Q) or UCI (g1f3), with or
// without move numbers.

import C from './board.js';
import { arrows, markers, pieces } from './sprites.js';

const still = matchMedia('(prefers-reduced-motion: reduce)').matches;

// The sprites the board would fetch, put where it looks for them first.
for (const [id, svg] of [['cm-chessboard-sprite', pieces], ['cm-chessboard-markers', markers], ['cm-chessboard-arrows', arrows]]) {
  if (document.getElementById(id)) continue;
  const div = document.createElement('div');
  div.id = id;
  div.setAttribute('aria-hidden', 'true');
  div.style.cssText = 'position: absolute; width: 0; height: 0; overflow: hidden';
  div.innerHTML = svg;
  document.body.append(div);
}

/** A FEN as chess.js wants it: six fields. */
function fullFen(fen) {
  let f = fen.trim().split(/\s+/);
  if (f.length < 2) f.push('w');
  while (f.length < 4) f.push('-');
  if (f.length < 6) f = f.slice(0, 4).concat(['0', '1']);
  return f.join(' ');
}

/** The answer's moves from `fen`, as chess.js moves, as far as they are legal. */
function line(fen, text) {
  const game = new C.Chess(fen);
  const out = [];
  for (const w of (text || '').split(/[\s,;]+/)) {
    const word = w.replace(/^\d+\.+/, '');
    if (!word || /^(\d+\.+|1-0|0-1|1\/2-1\/2|\*)$/.test(word)) continue;
    let mv = null;
    try {
      mv = game.move(/^[a-h][1-8][a-h][1-8][qrbn]?$/.test(word) ? { from: word.slice(0, 2), to: word.slice(2, 4), promotion: word[4] } : word);
    } catch { mv = null; }
    if (!mv) break;
    out.push(mv);
  }
  return out;
}

function text(tag, cls, content) {
  const el = document.createElement(tag);
  if (cls) el.className = cls;
  if (content) el.textContent = content;
  return el;
}

/** A board in `holder`. `onMove` hears each legal move the learner plays. */
function board(holder, fen, orient, onMove) {
  const game = new C.Chess(fen);
  const b = new C.Chessboard(holder, {
    position: fen,
    orientation: orient === 'b' ? C.COLOR.black : C.COLOR.white,
    assetsUrl: './',
    responsive: true,
    style: { cssClass: 'ainki', borderType: C.BORDER_TYPE.none, showCoordinates: true, animationDuration: still ? 0 : 180 },
    extensions: [
      { class: C.Markers, props: { autoMarkers: C.MARKER_TYPE.square } },
      { class: C.Arrows },
      { class: C.PromotionDialog },
    ],
  });
  if (!onMove) return { board: b, game, enable() {} };
  const play = (m) => {
    let mv = null;
    try { mv = game.move(m); } catch { mv = null; }
    if (!mv) return false;
    // Castling, en passant and promotion move more than the dragged piece.
    b.state.moveInputProcess.then(() => b.setPosition(game.fen(), !still));
    onMove(mv);
    return true;
  };
  const input = (e) => {
    const T = C.INPUT_EVENT_TYPE;
    if (e.type === T.moveInputStarted) {
      const moves = game.moves({ square: e.squareFrom, verbose: true });
      b.removeLegalMovesMarkers();
      b.addLegalMovesMarkers(moves);
      return moves.length > 0;
    }
    if (e.type === T.validateMoveInput) {
      b.removeLegalMovesMarkers();
      const promo = game.moves({ square: e.squareFrom, verbose: true }).some((m) => m.to === e.squareTo && m.promotion);
      if (!promo) return play({ from: e.squareFrom, to: e.squareTo });
      b.showPromotionDialog(e.squareTo, game.turn(), (r) => {
        if (r && r.type === C.PROMOTION_DIALOG_RESULT_TYPE.pieceSelected) {
          play({ from: e.squareFrom, to: e.squareTo, promotion: r.piece.charAt(1) });
        } else {
          b.setPosition(game.fen(), false);
        }
      });
      return true;
    }
    if (e.type === T.moveInputCanceled) b.removeLegalMovesMarkers();
    return true;
  };
  b.enableMoveInput(input);
  return { board: b, game, enable: () => b.enableMoveInput(input) };
}

/** The position, or a note saying why there is none. */
function start(fields, el) {
  const fen = fullFen(fields.fen || '');
  try {
    new C.Chess(fen);
  } catch {
    el.append(text('p', 'note', fields.fen ? 'This position is no valid FEN.' : 'Paste a position (FEN) to see the board.'));
    return null;
  }
  return fen;
}

function toMove(fen) {
  return fen.split(' ')[1] === 'b' ? 'Black to move' : 'White to move';
}

export function challenge(fields, el, respond) {
  const fen = start(fields, el);
  if (!fen) return;
  const answer = line(fen, fields.moves);
  const side = fen.split(' ')[1];
  const many = answer.length > 2;
  el.append(text('p', 'to-move', toMove(fen) + (many ? ': find the line' : '')));
  const holder = text('div', 'board');
  el.append(holder);
  const played = [];
  const b = board(holder, fen, side, (mv) => {
    played.push(mv.san);
    const i = played.length - 1;
    const right = answer[i] && answer[i].san === mv.san;
    const reply = answer[i + 1];
    // The line goes on while the learner follows it.
    if (right && reply && answer[i + 2]) {
      b.board.disableMoveInput();
      setTimeout(() => {
        b.game.move(reply.san);
        played.push(reply.san);
        b.board.setPosition(b.game.fen(), !still).then(b.enable);
      }, still ? 0 : 350);
      return;
    }
    b.board.disableMoveInput();
    setTimeout(() => respond({ moves: played }), still ? 0 : 250);
  });
}

export function solution(fields, response, el) {
  const fen = start(fields, el);
  if (!fen) return;
  const answer = line(fen, fields.moves);
  const mine = response && Array.isArray(response.moves) ? line(fen, response.moves.join(' ')) : [];
  el.append(text('p', 'to-move', toMove(fen)));
  const holder = text('div', 'board');
  el.append(holder);
  const b = board(holder, fen, fen.split(' ')[1], null);
  const game = new C.Chess(fen);
  const fens = [fen].concat(answer.map((m) => { game.move(m.san); return game.fen(); }));
  const buttons = [];
  const show = (n) => {
    b.board.setPosition(fens[n], !still && n > 0);
    b.board.removeArrows();
    b.board.removeMarkers();
    if (n === 0) {
      const first = answer[0];
      const wrong = mine[0] && (!first || mine[0].san !== first.san) ? mine[0] : null;
      if (wrong) b.board.addArrow(C.ARROW_TYPE.danger, wrong.from, wrong.to);
      if (first) b.board.addArrow(C.ARROW_TYPE.success, first.from, first.to);
    } else {
      const m = answer[n - 1];
      b.board.addMarker(C.MARKER_TYPE.square, m.from);
      b.board.addMarker(C.MARKER_TYPE.square, m.to);
    }
    buttons.forEach((btn, i) => btn.setAttribute('aria-pressed', String(i + 1 === n)));
  };
  const answerLine = text('p', 'line');
  answerLine.append(text('span', 'label', 'Answer'));
  answer.forEach((m, i) => {
    const btn = text('button', 'move', m.san);
    btn.type = 'button';
    btn.addEventListener('click', () => show(btn.getAttribute('aria-pressed') === 'true' ? 0 : i + 1));
    buttons.push(btn);
    answerLine.append(btn);
  });
  if (!answer.length) answerLine.append(text('span', 'move', fields.moves || ''));
  el.append(answerLine);
  if (mine.length) {
    const yours = text('p', 'line yours');
    yours.append(text('span', 'label', 'You played'));
    mine.forEach((m, i) => yours.append(text('span', answer[i] && answer[i].san === m.san ? 'move same' : 'move other', m.san)));
    el.append(yours);
  }
  if (fields.note) el.append(text('p', 'note', fields.note));
  show(0);
}
