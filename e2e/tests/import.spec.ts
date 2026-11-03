// Bringing cards in: pasted CSV, CSV files, and Anki packages, the way
// someone moving from Anki would.
import { test, expect, sql, REFUSED } from '../support/fixtures.ts';
import { flash } from '../support/ui.ts';
import { apkg, png } from '../support/files.ts';

test('pasted CSV lands in the chosen deck', async ({ page, seed }) => {
  await seed.deck('First');
  const target = await seed.deck('Second');
  await page.goto('/import');
  const form = page.locator('form:has(input[name=kind][value=csv])');
  await form.locator('select[name=deck]').selectOption(String(target));
  await form.locator('textarea[name=text]').fill('front,back\nHund,dog\n"Katze, die",cat\n,no front\n');
  await form.getByRole('button', { name: 'Import CSV' }).click();
  await expect(page).toHaveURL('/');
  await expect(flash(page)).toHaveText('Imported 2 cards. Skipped 1 empty.');
  expect(sql('SELECT front, back FROM cards WHERE deck_id = ? ORDER BY id', target)).toEqual([
    { front: 'Hund', back: 'dog' },
    { front: 'Katze, die', back: 'cat' },
  ]);
});

test('a CSV file with tabs and UTF-8 works too', async ({ page, seed }) => {
  const deck = await seed.deck('Tabs');
  await page.goto('/import');
  const form = page.locator('form:has(input[name=kind][value=csv])');
  await form.locator('input[type=file]').setInputFiles({
    name: 'words.tsv',
    mimeType: 'text/tab-separated-values',
    buffer: Buffer.from('Straße\tstreet\n日本\tJapan\nçà\tunicode\n'),
  });
  await form.getByRole('button', { name: 'Import CSV' }).click();
  await expect(flash(page)).toHaveText('Imported 3 cards.');
  expect(sql<{ front: string }>('SELECT front FROM cards WHERE deck_id = ? ORDER BY id', deck).map((r) => r.front)).toEqual(['Straße', '日本', 'çà']);
});

test('the CSV form waits for a first deck', async ({ page }) => {
  await page.goto('/import');
  await expect(page.locator('form:has(input[name=kind][value=csv])')).toHaveCount(0);
  await expect(page.locator('main')).toContainText('Create a deck first.');
});

test('an Anki package brings decks, schedule and images', async ({ page, account }) => {
  const file = apkg(
    [
      { deck: 'Languages::Spanish', fields: ['el <b>perro</b>', 'the dog'] },
      { deck: 'Languages::Spanish', fields: ['el gato', 'the cat'], review: { ivl: 20, dueInDays: 4 } },
      { deck: 'Languages::Spanish', fields: ['la luna', 'the moon'], suspended: true },
      { deck: 'Flags', fields: ['<img src="es.png">', 'Spain'] },
    ],
    { 'es.png': png(30, 20) },
  );
  await page.goto('/import');
  const form = page.locator('form:has(input[name=kind][value=apkg])');
  await form.locator('input[type=file]').setInputFiles({ name: 'export.apkg', mimeType: 'application/octet-stream', buffer: file });
  await form.getByRole('button', { name: 'Import Anki deck' }).click();
  await expect(page).toHaveURL('/');
  await expect(flash(page)).toHaveText('Imported 4 cards, 1 image. New decks: Languages › Spanish, Flags.');

  const cards = sql<{ front: string; state: number; suspended: number; front_image: string | null; due: number }>(
    'SELECT front, state, suspended, front_image, due FROM cards WHERE user_id = ? ORDER BY id',
    account.id,
  );
  expect(cards.map((c) => c.front)).toEqual(['el perro', 'el gato', 'la luna', '']);
  expect(cards[1].state).toBe(2);
  expect(cards[1].due).toBeGreaterThan(Date.now() / 1000 + 2 * 86_400);
  expect(cards[2].suspended).toBe(1);
  expect(cards[3].front_image).toBeTruthy();

  // Today: the new cards only (the review is days away, one is suspended).
  await page.goto('/study');
  await expect(page.locator('main .study-meta .count')).toHaveText('1 of 2');
});

test('a file that is not an Anki package says so', async ({ page, account, allowErrors }) => {
  allowErrors.push(REFUSED);
  await page.goto('/import');
  const form = page.locator('form:has(input[name=kind][value=apkg])');
  await form.locator('input[type=file]').setInputFiles({ name: 'holiday.apkg', mimeType: 'application/octet-stream', buffer: Buffer.from('not a zip at all') });
  await form.getByRole('button', { name: 'Import Anki deck' }).click();
  await expect(page).toHaveURL('/import');
  await expect(flash(page)).toContainText(/not a valid \.apkg file/i);
  expect(sql('SELECT 1 FROM decks WHERE user_id = ?', account.id)).toHaveLength(0);
});
