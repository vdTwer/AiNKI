// Card types people bring: the chess type, uploaded as a .zip on the Card
// types page, writes a card; studying it, the move played turns the card
// over and is kept with the rating.
import { test, expect, sql } from '../support/fixtures.ts';
import { flash } from '../support/ui.ts';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const RUY = 'r1bqkbnr/pppp1ppp/2n5/4p3/4P3/5N2/PPPP1PPP/RNBQKB1R w KQkq - 2 3';

test('upload the chess type, write a card, play the move', async ({ page, seed, account }) => {
  const deck = await seed.deck('Tactics');
  // The example the page offers, uploaded as anyone would.
  const zip = await (await page.request.get('/settings/types/chess.zip')).body();
  const file = path.join(fs.mkdtempSync(path.join(os.tmpdir(), 'ainki-')), 'chess.zip');
  fs.writeFileSync(file, zip);
  await page.goto('/settings/types');
  await page.locator('input[type=file]').setInputFiles(file);
  await page.getByRole('button', { name: 'Upload' }).click();
  await expect(flash(page)).toHaveText('Chess version 1 is ready.');

  await page.getByRole('link', { name: 'Add card' }).click();
  await page.locator('select[name=deck]').selectOption(String(deck));
  await page.locator('#field-fen').fill(RUY);
  await page.locator('#field-moves').fill('Bb5');
  // The form draws the card as it is written.
  const preview = page.frameLocator('.type-preview iframe');
  await expect(preview.locator('.to-move')).toHaveText('White to move');
  await page.getByRole('button', { name: /add card/i }).click();
  await expect(flash(page)).toHaveText('Card added.');
  const [card] = sql<{ id: number; front: string; fields: string }>(
    'SELECT id, front, fields FROM cards WHERE user_id = ?',
    account.id,
  );
  expect(card.front).toBe(`Chess: ${RUY}`);
  expect(JSON.parse(card.fields)).toEqual({ fen: RUY, moves: 'Bb5' });

  await page.goto('/study');
  const board = page.frameLocator('iframe.type-frame').locator('.board svg').first();
  await expect(board).toBeVisible();
  const box = (await board.boundingBox())!;
  const w = box.width / 8;
  const at = (file: number, rank: number) => ({ x: box.x + (file + 0.5) * w, y: box.y + (8 - rank + 0.5) * w });
  for (const sq of [at(5, 1), at(1, 5)]) {
    await page.mouse.click(sq.x, sq.y); // f1, then b5
    await page.waitForTimeout(100);
  }
  await expect(page.locator('.study')).toHaveClass(/revealed/);
  await expect(page.frameLocator('iframe.type-frame').locator('.line.yours')).toContainText('Bb5');
  await page.keyboard.press('3');
  await expect(page.locator('main .session-end')).toBeVisible();
  const [review] = sql<{ rating: number; response: string }>('SELECT rating, response FROM review_log WHERE card_id = ?', card.id);
  expect(review).toEqual({ rating: 3, response: '{"moves":["Bb5"]}' });
});

test('a file that is no package is refused', async ({ page, allowErrors }) => {
  allowErrors.push(/status of 400/);
  await page.goto('/settings/types');
  const file = path.join(fs.mkdtempSync(path.join(os.tmpdir(), 'ainki-')), 'bad.zip');
  fs.writeFileSync(file, 'not a zip');
  await page.locator('input[type=file]').setInputFiles(file);
  await page.getByRole('button', { name: 'Upload' }).click();
  await expect(page.locator('main')).toContainText('This is no .zip file');
});
