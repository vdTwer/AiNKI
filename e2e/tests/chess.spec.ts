// Chess cards: the front is a position, the back the move that answers it.
// The first real move on the board turns the card and grades it (right is
// Good, wrong is Again); arrows never count.
import { test, expect, sql, logIn, type Seed } from '../support/fixtures.ts';
import { flash } from '../support/ui.ts';
import type { Page } from '@playwright/test';

/** White mates with Qxf7# (the Scholar's mate). */
const SCHOLAR = 'r1bqkb1r/pppp1ppp/2n2n2/4p2Q/2B1P3/8/PPPP1PPP/RNB1K1NR w KQkq - 4 4';
/** Black to move: the board turns round. */
const BLACK = 'rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1';

async function chessCard(seed: Seed, deck: number, front: string, back: string): Promise<number> {
  return seed.card(deck, front, back, { kind: 'chess' });
}

const square = (page: Page, sq: string) => page.locator(`main .chess-board .cm-chessboard rect.square[data-square="${sq}"]`);

/** A move by clicking its two squares, as on a touch screen. */
async function play(page: Page, from: string, to: string) {
  await square(page, from).click();
  await square(page, to).click();
}

async function reviews(card: number) {
  return sql<{ rating: number }>('SELECT rating FROM review_log WHERE card_id = ? ORDER BY id', card).map((r) => r.rating);
}

test('a right move turns the card and counts as Good', async ({ page, seed }) => {
  const deck = await seed.deck('Tactics');
  const card = await chessCard(seed, deck, `${SCHOLAR}\n\nMate in one`, 'Qxf7#\n\nThe classic trap: f7 is guarded by the king alone.');
  await page.goto('/study');
  await page.evaluate(() => ((window as unknown as { __kept: number }).__kept = 1));
  await expect(page.locator('main .chess-front')).toContainText('White to move');
  await expect(page.locator('main .chess-front')).toContainText('Mate in one');
  // The board replaces the typed move.
  await expect(square(page, 'h5')).toBeVisible();
  await expect(page.locator('main .typed-move')).toBeHidden();

  await play(page, 'h5', 'f7');
  await expect(page.locator('main .chess-back .verdict')).toHaveText('Right: Qxf7#.');
  await expect(page.locator('main .chess-back')).toContainText('guarded by the king alone');
  await expect.poll(() => reviews(card)).toEqual([3]);
  expect(await page.evaluate(() => (window as unknown as { __kept?: number }).__kept), 'no reload').toBe(1);

  await page.keyboard.press(' ');
  await expect(page.locator('main .session-end')).toBeVisible();
  expect(await page.evaluate(() => (window as unknown as { __kept?: number }).__kept), 'no reload').toBe(1);
});

test('a wrong move counts as Again and shows the right one', async ({ page, seed }) => {
  const deck = await seed.deck('Tactics');
  const card = await chessCard(seed, deck, SCHOLAR, 'Qxf7#');
  await page.goto('/study');
  await play(page, 'b1', 'c3');
  await expect(page.locator('main .chess-back .verdict')).toHaveText('Not this time. You played Nc3.');
  await expect(page.locator('main .chess-back .also')).toHaveText('The move is Qxf7#.');
  // Both moves stand on the board as arrows.
  await expect(page.locator('main .cm-chessboard .arrow-success')).toHaveCount(1);
  await expect(page.locator('main .cm-chessboard .arrow-danger')).toHaveCount(1);
  await expect.poll(() => reviews(card)).toEqual([1]);
});

test('arrows never count as a move', async ({ page, seed }) => {
  const deck = await seed.deck('Tactics');
  const card = await chessCard(seed, deck, SCHOLAR, 'Qxf7#');
  await page.goto('/study');
  const from = await square(page, 'h5').boundingBox();
  const to = await square(page, 'f7').boundingBox();
  if (!from || !to) throw new Error('no board');
  await page.mouse.move(from.x + from.width / 2, from.y + from.height / 2);
  await page.mouse.down({ button: 'right' });
  await page.mouse.move(to.x + to.width / 2, to.y + to.height / 2, { steps: 5 });
  await page.mouse.up({ button: 'right' });
  await expect(page.locator('main .cm-chessboard .arrows .arrow')).toHaveCount(1);
  await page.waitForTimeout(300);
  expect(await reviews(card)).toEqual([]);
  await expect(page.locator('main .chess-back')).toHaveCount(0);
});

test('Black to move: the board turns, and undo asks again', async ({ page, seed }) => {
  const deck = await seed.deck('Openings');
  const card = await chessCard(seed, deck, BLACK, 'c7c5, e7e5\n\nThe Sicilian or the open game.');
  await page.goto('/study');
  await expect(page.locator('main .chess-front')).toContainText('Black to move');
  await expect(page.locator('main .cm-chessboard .board')).toBeVisible();
  // Black plays up the page: a8 is at the bottom.
  const a8 = await square(page, 'a8').boundingBox();
  const a1 = await square(page, 'a1').boundingBox();
  expect(a8!.y).toBeGreaterThan(a1!.y);
  await play(page, 'c7', 'c5');
  await expect(page.locator('main .chess-back .verdict')).toHaveText('Right: c5.');
  await expect(page.locator('main .chess-back .also')).toHaveText('Right moves: c5 or e5.');
  await expect.poll(() => reviews(card)).toEqual([3]);

  // The board stays free to explore, and goes back to the start.
  await expect(page.locator('main .board-tools')).toBeHidden();
  await play(page, 'g1', 'f3');
  await expect(page.locator('main .cm-chessboard g[data-piece=wn][data-square=f3]')).toHaveCount(1);
  await expect(page.locator('main .board-tools')).toBeVisible();
  await page.locator('main .board-reset').click();
  await expect(page.locator('main .board-tools')).toBeHidden();
  await expect(page.locator('main .cm-chessboard g[data-piece=wn][data-square=g1]')).toHaveCount(1);

  await page.getByRole('button', { name: /undo last answer/i }).click();
  await expect(page.locator('main .chess-back')).toHaveCount(0);
  await expect(square(page, 'e7')).toBeVisible();
  await expect.poll(() => reviews(card)).toEqual([]);
});

test('without JavaScript the move is typed', async ({ browser, seed, account }) => {
  const deck = await seed.deck('Tactics');
  const card = await chessCard(seed, deck, SCHOLAR, 'Qxf7#');
  const context = await browser.newContext({ javaScriptEnabled: false });
  await logIn(context.request, account);
  const page = await context.newPage();
  await page.goto('/study');
  await expect(page.locator('main .plain-board .sq')).toHaveCount(64);
  // A move that isn't legal changes nothing.
  await page.locator('main input[name=played]').fill('Qxf8');
  await page.getByRole('button', { name: 'Play' }).click();
  await expect(flash(page)).toContainText("isn't a legal move");
  expect(await reviews(card)).toEqual([]);
  // UCI works as well as SAN.
  await page.locator('main input[name=played]').fill('h5f7');
  await page.getByRole('button', { name: 'Play' }).click();
  await expect(page.locator('main .chess-back .verdict')).toHaveText('Right: Qxf7#.');
  expect(await reviews(card)).toEqual([3]);
  await context.close();
});

test('asking for the move counts as Again', async ({ page, seed }) => {
  const deck = await seed.deck('Tactics');
  const card = await chessCard(seed, deck, SCHOLAR, 'Qxf7#');
  await page.goto('/study');
  await page.getByRole('button', { name: 'Show the move' }).click();
  await expect(page.locator('main .chess-back .verdict')).toHaveText('The move is Qxf7#.');
  await expect.poll(() => reviews(card)).toEqual([1]);
});

test('writing a chess card: the move is played on the board', async ({ page, seed, account }) => {
  const deck = await seed.deck('Tactics');
  await page.goto(`/cards/new?deck=${deck}`);
  await page.locator('select[name=kind]').selectOption('chess');
  await expect(page.locator('label[for=front]')).toHaveText('Position');
  await page.locator('#front').fill(`${SCHOLAR}\n\nMate in one`);
  const sq = (s: string) => page.locator(`.chess-edit rect.square[data-square="${s}"]`);
  await expect(sq('h5')).toBeVisible();
  await sq('h5').click();
  await sq('f7').click();
  await expect(page.locator('#back')).toHaveValue('Qxf7#');
  await expect(page.locator('.chess-edit .arrow-success')).toHaveCount(1);
  const queenHome = page.locator('.chess-edit g[data-piece=wq][data-square=h5]');
  await expect(queenHome).toHaveCount(1);
  // A second right move, then taking it back again.
  await sq('h5').click();
  await sq('h7').click();
  await expect(page.locator('#back')).toHaveValue('Qxf7#, Qxh7');
  await expect(queenHome).toHaveCount(1);
  await sq('h5').click();
  await sq('h7').click();
  await expect(page.locator('#back')).toHaveValue('Qxf7#');
  await page.locator('#back').press('End');
  await page.locator('#back').pressSequentially('\n\nf7 falls.');
  await page.getByRole('button', { name: /add card/i }).click();
  await expect(flash(page)).toHaveText('Card added.');
  // The next card is a chess card too.
  await expect(page.locator('select[name=kind]')).toHaveValue('chess');
  const [card] = sql<{ kind: string; front: string; back: string }>('SELECT kind, front, back FROM cards WHERE user_id = ?', account.id);
  expect(card).toEqual({ kind: 'chess', front: `${SCHOLAR}\n\nMate in one`, back: 'Qxf7#\n\nf7 falls.' });
});

test('a chess card that does not fit comes back with the problem', async ({ page, seed, allowErrors }) => {
  allowErrors.push(/status of 4\d\d/);
  const deck = await seed.deck('Tactics');
  await page.goto(`/cards/new?deck=${deck}&kind=chess`);
  await page.locator('#front').fill(SCHOLAR);
  await page.locator('#back').fill('Qxf8');
  await page.getByRole('button', { name: /add card/i }).click();
  await expect(page.locator('main')).toContainText("“Qxf8” isn't a legal move in this position");
  await expect(page.locator('#back')).toHaveValue('Qxf8');
});

test('importing positions: EPD, FEN with moves, and the lines that fail', async ({ page, seed, account }) => {
  const deck = await seed.deck('Puzzles');
  await page.goto('/import');
  const form = page.locator('form:has(input[name=kind][value=chess])');
  await form.locator('select[name=deck]').selectOption(String(deck));
  await form.locator('textarea').fill(
    [
      `${SCHOLAR} bm Qxf7#; c0 "Scholar's mate";`,
      `${BLACK} c5, e5; Two good answers`,
      'not a position at all',
    ].join('\n'),
  );
  await form.getByRole('button', { name: 'Import positions' }).click();
  await expect(flash(page)).toContainText('Imported 2 cards');
  await expect(flash(page)).toContainText('line 3');
  const cards = sql<{ kind: string; back: string }>('SELECT kind, back FROM cards WHERE user_id = ? ORDER BY id', account.id);
  expect(cards).toEqual([
    { kind: 'chess', back: "Qxf7#\n\nScholar's mate" },
    { kind: 'chess', back: 'c5, e5\n\nTwo good answers' },
  ]);
});
