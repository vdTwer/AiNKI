// Multicards: one card whose back splits into items, each remembered on
// its own. Tap what you remembered; the rest come back sooner.
import { test, expect, sql } from '../support/fixtures.ts';
import { flash } from '../support/ui.ts';

const TENSES = '**will**\n\n- spontaneous decisions\n- predictions without evidence\n\n**be going to**\n\n- plans\n- predictions with evidence\n\n⚠️ Never in time clauses.';

test('the card form offers only the splits the back allows', async ({ page, seed, account }) => {
  const deck = await seed.deck('Grammar');
  await page.goto(`/cards/new?deck=${deck}`);
  const pick = (how: string) => page.locator(`.multi-pick [data-split=${how}]`);
  await page.locator('#front').fill('Future simple: when is it used?');
  await page.locator('#back').fill('just one answer');
  await expect(pick('list')).toBeHidden();

  await page.locator('#back').fill(TENSES);
  await expect(pick('list')).toBeVisible();
  await expect(pick('parts')).toBeHidden();
  await pick('list').locator('input').check();

  // The preview shows the items, with the text between them kept.
  await page.locator('.preview-toggle').click();
  await expect(page.locator('.card-preview')).toContainText('Never in time clauses');
  await expect(page.locator('.card-preview li')).toHaveCount(4);
  await page.locator('.preview-toggle').click();

  await page.getByRole('button', { name: /add card/i }).click();
  await expect(flash(page)).toHaveText('Card added.');
  const [card] = sql<{ id: number; multi: string }>('SELECT id, multi FROM cards WHERE user_id = ?', account.id);
  expect(card.multi).toBe('list');
  expect(sql<{ text: string }>('SELECT text FROM card_items WHERE card_id = ? ORDER BY position', card.id).map((i) => i.text)).toEqual([
    'spontaneous decisions',
    'predictions without evidence',
    'plans',
    'predictions with evidence',
  ]);
});

test('studying a multicard: tap what you remembered', async ({ page, seed, account }) => {
  const deck = await seed.deck('Grammar');
  const card = await seed.card(deck, 'Future simple: when is it used?', TENSES, { multi: 'list' });
  await page.goto('/study');
  await page.evaluate(() => ((window as unknown as { __kept: number }).__kept = 1));
  const form = page.locator('main form.list-answer');
  await expect(form.locator('.face.back')).toBeHidden();
  await page.keyboard.press(' ');
  await expect(form.locator('.face.back')).toBeVisible();
  // Group labels and the closing remark are shown with the items.
  await expect(form.locator('.face.back')).toContainText('be going to');
  await expect(form.locator('.face.back')).toContainText('Never in time clauses');

  const done = form.getByRole('button', { name: /done/i });
  await expect(done).toContainText('0 of 4 remembered');
  await form.locator('label.item').nth(0).click();
  await page.keyboard.press('3'); // the third item, by key
  await expect(done).toContainText('2 of 4 remembered');
  await done.click();
  await expect(page.locator('main .session-end, main .study')).toBeVisible();
  expect(await page.evaluate(() => (window as unknown as { __kept?: number }).__kept)).toBe(1);

  const [log] = sql<{ items_asked: number; items_recalled: number }>(
    'SELECT items_asked, items_recalled FROM review_log WHERE card_id = ?',
    card,
  );
  expect(log).toEqual({ items_asked: 4, items_recalled: 2 });
  const items = sql<{ position: number; rating: number }>(
    'SELECT ci.position, r.rating FROM item_review_log r JOIN card_items ci ON ci.id = r.item_id WHERE ci.card_id = ? ORDER BY ci.position',
    card,
  );
  expect(items.map((i) => i.rating)).toEqual([3, 1, 3, 1]);
  void account;
});

test('while relearning, only the forgotten items are asked', async ({ page, seed }) => {
  const deck = await seed.deck('Lists');
  const card = await seed.card(deck, 'Primary colours', '- red\n- yellow\n- blue', { multi: 'list' });
  // Red and yellow are known for weeks; blue was forgotten and is due.
  sql(
    `UPDATE card_items SET state = 2, stability = 30, difficulty = 5, reps = 3,
       last_review = unixepoch() - 86400, due = unixepoch() + 20 * 86400
     WHERE card_id = ? AND position < 2`,
    card,
  );
  sql('UPDATE card_items SET state = 3, stability = 1, difficulty = 7, reps = 3, lapses = 1, last_review = unixepoch() - 600, due = unixepoch() - 60 WHERE card_id = ? AND position = 2', card);
  sql('UPDATE cards SET state = 3, reps = 3, lapses = 1, due = unixepoch() - 60, stability = 1, last_review = unixepoch() - 600 WHERE id = ?', card);

  await page.goto('/study');
  const known = page.locator('main .list-known');
  await expect(known).toContainText('You know');
  await expect(known).toContainText('red');
  await expect(known).toContainText('and 1 more?');
  await page.locator('main .study .reveal a.show, main .reveal a.show').first().click();
  await expect(page.locator('main form.list-answer label.item')).toHaveCount(1);
  await expect(page.locator('main form.list-answer label.item')).toContainText('blue');
});

test('without JavaScript the list is one plain form', async ({ browser, page: loggedIn, seed }) => {
  const deck = await seed.deck('Lists');
  await seed.card(deck, 'Primary colours', '- red\n- yellow\n- blue', { multi: 'list' });
  // The same account, scripts off.
  const context = await browser.newContext({ javaScriptEnabled: false, storageState: await loggedIn.context().storageState() });
  const page = await context.newPage();
  await page.goto('/study');
  await page.locator('main .reveal a.show').click();
  await page.locator('main form.list-answer label.item').nth(1).click();
  await page.locator('main form.list-answer').getByRole('button', { name: /done/i }).click();
  await expect(page.locator('main')).toBeVisible();
  expect(sql<{ items_recalled: number }>('SELECT items_recalled FROM review_log r JOIN cards c ON c.id = r.card_id WHERE c.deck_id = ?', deck)[0].items_recalled).toBe(1);
  await context.close();
});
