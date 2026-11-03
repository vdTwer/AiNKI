// Making decks and cards, editing them, images, Markdown and the card's
// own actions (suspend, forget, delete).
import { test, expect, sql, desktopOnly, REFUSED } from '../support/fixtures.ts';
import { fillDeckName, flash, open } from '../support/ui.ts';
import { png } from '../support/files.ts';

test('a new deck goes straight on to its first card', async ({ page, account }) => {
  await page.goto('/decks/new');
  const form = page.locator('form[action="/decks/new"]');
  await fillDeckName(form, 'Languages::Spanish');
  await form.locator('textarea[name=description]').fill('Verbs and nouns');
  await form.locator('input[name=preset_id]').nth(2).check({ force: true });
  await form.locator('input[name=exam_date]').fill('2099-06-30');
  await form.getByRole('button', { name: 'Create deck' }).click();

  await expect(page).toHaveURL(/\/cards\/new\?deck=\d+/);
  await expect(flash(page)).toHaveText('Deck created. Add its first card.');
  const [deck] = sql<{ name: string; exam_date: string; preset: string }>(
    'SELECT d.name, d.exam_date, p.name AS preset FROM decks d JOIN presets p ON p.id = d.preset_id WHERE d.user_id = ?',
    account.id,
  );
  expect(deck).toEqual({ name: 'Languages::Spanish', exam_date: '2099-06-30', preset: 'Language vocabulary' });

  // The picker already has the new deck, shown as a path.
  await expect(page.locator('select[name=deck] option:checked')).toHaveText('Languages › Spanish');
  await page.locator('#front').fill('el perro');
  await page.locator('#back').fill('the dog');
  await page.getByRole('button', { name: /add card/i }).click();
  await expect(flash(page)).toHaveText('Card added.');
  // Ready for the next one, same deck, empty sides.
  await expect(page.locator('#front')).toHaveValue('');
  await expect(page.locator('select[name=deck] option:checked')).toHaveText('Languages › Spanish');

  // Names with "::" are never shown raw.
  for (const path of ['/', '/stats']) {
    await page.goto(path);
    await expect(page.locator('main')).not.toContainText('::');
  }
});

test.describe('desktop', () => {
  desktopOnly();
  test('Ctrl+Enter adds a card', async ({ page, seed }) => {
  const deck = await seed.deck('Quick');
  await page.goto(`/cards/new?deck=${deck}`);
  await page.locator('#front').fill('Ctrl front');
  await page.locator('#back').fill('Ctrl back');
  await page.locator('#back').press('Control+Enter');
  await expect(flash(page)).toHaveText('Card added.');
  expect(sql('SELECT 1 FROM cards WHERE deck_id = ? AND front = ?', deck, 'Ctrl front')).toHaveLength(1);
  });
});

test('"New deck…" in the card form makes the deck too', async ({ page, seed, account }) => {
  await seed.deck('Existing');
  await page.goto('/cards/new');
  await expect(page.locator('#new-deck-field')).toBeHidden();
  await page.locator('select[name=deck]').selectOption('new');
  await expect(page.locator('#new-deck-field')).toBeVisible();
  await fillDeckName(page.locator('#new-deck-field'), 'Fresh');
  await page.locator('#front').fill('q');
  await page.locator('#back').fill('a');
  await page.getByRole('button', { name: /add card/i }).click();
  await expect(flash(page)).toHaveText('Deck created and card added.');
  expect(sql('SELECT name FROM decks WHERE user_id = ? ORDER BY id', account.id)).toEqual([{ name: 'Existing' }, { name: 'Fresh' }]);

  // Naming a deck that exists adds to it and says so.
  await page.goto('/cards/new');
  await page.locator('select[name=deck]').selectOption('new');
  await fillDeckName(page.locator('#new-deck-field'), 'Existing');
  await page.locator('#front').fill('q2');
  await page.locator('#back').fill('a2');
  await page.getByRole('button', { name: /add card/i }).click();
  await expect(flash(page)).toHaveText('Card added to your existing deck Existing.');
});

test('a card without a front is refused and keeps what was typed', async ({ page, seed, allowErrors }) => {
  allowErrors.push(REFUSED);
  const deck = await seed.deck('Strict');
  await page.goto(`/cards/new?deck=${deck}`);
  await page.locator('#back').fill('an answer with no question');
  // The browser would stop an empty required field; send it anyway.
  await page.locator('#front').evaluate((t) => t.removeAttribute('required'));
  await page.getByRole('button', { name: /add card/i }).click();
  await expect(page.locator('#back')).toHaveValue('an answer with no question');
  expect(sql('SELECT 1 FROM cards WHERE deck_id = ?', deck)).toHaveLength(0);
});

test('an image goes on the card and is served back', async ({ page, seed }) => {
  const deck = await seed.deck('Pictures');
  await page.goto(`/cards/new?deck=${deck}`);
  await page.locator('input[name=front_image]').setInputFiles({ name: 'flag.png', mimeType: 'image/png', buffer: png(40, 30) });
  await expect(page.locator('.face-edit').first().locator('.face-image.preview img')).toBeVisible();
  await page.locator('#back').fill('the flag');
  // The front has an image, so its text may stay empty.
  await page.locator('#front').evaluate((t) => t.removeAttribute('required'));
  await page.getByRole('button', { name: /add card/i }).click();
  await expect(flash(page)).toHaveText('Card added.');

  const [card] = sql<{ id: number; front_image: string }>('SELECT id, front_image FROM cards WHERE deck_id = ?', deck);
  expect(card.front_image).toBeTruthy();
  const res = await page.request.get(`/media/${card.front_image}`);
  expect(res.status()).toBe(200);
  expect(res.headers()['content-type']).toBe('image/png');

  // Studying shows it, loaded.
  await page.goto(`/study?deck=${deck}`);
  const img = page.locator('main .study .face.front img');
  await expect(img).toBeVisible();
  expect(await img.evaluate((i) => (i as HTMLImageElement).naturalWidth)).toBeGreaterThan(0);

  // Removing it from the card: then the front needs words.
  await page.goto(`/cards/${card.id}`);
  await page.locator('input[name=remove_front_image]').check({ force: true });
  await page.locator('#front').fill('Which flag?');
  await page.getByRole('button', { name: /^save/i }).click();
  expect(sql<{ front_image: string | null }>('SELECT front_image FROM cards WHERE id = ?', card.id)[0].front_image).toBeNull();
});

test('a garbage "image" is refused', async ({ page, seed, allowErrors }) => {
  // The preview tries to show it first, and fails.
  allowErrors.push(REFUSED, /blob:.*Content Security Policy/s);
  const deck = await seed.deck('Pictures');
  await page.goto(`/cards/new?deck=${deck}`);
  await page.locator('#front').fill('front');
  await page.locator('#back').fill('back');
  await page.locator('input[name=back_image]').setInputFiles({ name: 'evil.png', mimeType: 'image/png', buffer: Buffer.from('<script>alert(1)</script>') });
  await page.getByRole('button', { name: /add card/i }).click();
  await expect(page.locator('main')).toContainText(/image/i);
  expect(sql('SELECT 1 FROM cards WHERE deck_id = ?', deck)).toHaveLength(0);
  // What was typed is still there.
  await expect(page.locator('#front')).toHaveValue('front');
});

test('Markdown preview matches what is studied', async ({ page, seed }) => {
  const deck = await seed.deck('Markdown');
  await page.goto(`/cards/new?deck=${deck}`);
  await page.locator('#front').fill('What is **bold**?');
  await page.locator('#back').fill('*Italic* and `code`\n\n<script>window.hacked = 1</script>');
  await page.locator('.preview-toggle').click();
  const preview = page.locator('.card-preview');
  await expect(preview.locator('strong')).toHaveText('bold');
  await expect(preview.locator('em')).toHaveText('Italic');
  await expect(preview.locator('code')).toHaveText('code');
  await page.locator('.preview-toggle').click();
  await page.getByRole('button', { name: /add card/i }).click();

  await page.goto(`/study?deck=${deck}`);
  await expect(page.locator('main .face.front strong')).toHaveText('bold');
  await page.locator('main .study .reveal a.show').click();
  await expect(page.locator('main .face.back em')).toHaveText('Italic');
  expect(await page.evaluate(() => (window as unknown as { hacked?: number }).hacked)).toBeUndefined();
});

test('editing, suspending, forgetting and deleting a card', async ({ page, seed }) => {
  const deck = await seed.deck('Edit me');
  const id = await seed.card(deck, 'Hund', 'dog');
  sql('UPDATE cards SET state = 2, stability = 5, difficulty = 5, reps = 3, due = unixepoch() + 86400 WHERE id = ?', id);

  await page.goto(`/decks/${deck}`);
  await page.locator(`a.row[href="/cards/${id}"]`).click();
  await expect(page).toHaveURL(`/cards/${id}`);
  await page.locator('#back').fill('dog (der)');
  await page.getByRole('button', { name: /^save/i }).click();
  expect(sql<{ back: string }>('SELECT back FROM cards WHERE id = ?', id)[0].back).toBe('dog (der)');

  await page.goto(`/cards/${id}`);
  await page.getByRole('button', { name: 'Suspend' }).click();
  expect(sql<{ suspended: number }>('SELECT suspended FROM cards WHERE id = ?', id)[0].suspended).toBe(1);
  await page.goto(`/decks/${deck}?state=suspended`);
  await expect(page.locator(`a.row[href="/cards/${id}"]`)).toBeVisible();
  await page.goto(`/cards/${id}`);
  await page.getByRole('button', { name: /unsuspend|resume/i }).click();
  expect(sql<{ suspended: number }>('SELECT suspended FROM cards WHERE id = ?', id)[0].suspended).toBe(0);

  await page.goto(`/cards/${id}`);
  await open(page.locator('details.danger'));
  await page.getByRole('button', { name: 'Forget progress' }).click();
  expect(sql<{ state: number; reps: number }>('SELECT state, reps FROM cards WHERE id = ?', id)[0]).toEqual({ state: 0, reps: 0 });

  await page.goto(`/cards/${id}`);
  await open(page.locator('details.danger'));
  await page.getByRole('button', { name: 'Delete card' }).click();
  expect(sql('SELECT 1 FROM cards WHERE id = ?', id)).toHaveLength(0);
  await expect(page).toHaveURL(new RegExp(`/decks/${deck}`));
});

test('deck page: search, filter, edit, export and delete', async ({ page, seed, account }) => {
  const deck = await seed.deck('History');
  await seed.cards(deck, [['Hastings', '1066'], ['Waterloo', '1815'], ['Agincourt', '1415']]);

  await page.goto(`/decks/${deck}`);
  await expect(page.locator('main a.row')).toHaveCount(3);
  await page.locator('input[name=q]').fill('water');
  await page.locator('form.search').getByRole('button', { name: 'Find' }).click();
  await expect(page.locator('main a.row')).toHaveCount(1);
  await expect(page.locator('main a.row')).toContainText('Waterloo');

  // Export: a CSV named after the deck.
  const csv = await page.request.get(`/decks/${deck}/export.csv`);
  expect(csv.headers()['content-disposition']).toContain('History.csv');
  expect(await csv.text()).toContain('Waterloo,1815');

  // Edit: rename into a path and set an exam date.
  await page.goto(`/decks/${deck}/edit`);
  const form = page.locator(`form[action="/decks/${deck}/edit"]`);
  await fillDeckName(form, 'School::History');
  await form.locator('input[name=exam_date]').fill('2099-01-15');
  await form.getByRole('button', { name: 'Save' }).click();
  expect(sql('SELECT name, exam_date FROM decks WHERE id = ?', deck)).toEqual([{ name: 'School::History', exam_date: '2099-01-15' }]);

  // Delete: behind a fold and a checkbox.
  await page.goto(`/decks/${deck}/edit`);
  await open(page.locator('details.danger'));
  const del = page.locator(`form[action="/decks/${deck}/delete"]`);
  await del.getByRole('button', { name: 'Delete deck' }).click();
  // The checkbox is required: nothing happened yet.
  expect(sql('SELECT 1 FROM decks WHERE id = ?', deck)).toHaveLength(1);
  await del.locator('input[name=confirm]').check({ force: true });
  await del.getByRole('button', { name: 'Delete deck' }).click();
  await expect(page).toHaveURL('/');
  await expect(flash(page)).toHaveText('Deck deleted, with its 3 cards.');
  expect(sql('SELECT 1 FROM cards WHERE user_id = ?', account.id)).toHaveLength(0);
});

test("other people's decks and cards stay out of reach", async ({ page, seed, browser }) => {
  const deck = await seed.deck('Private');
  const card = await seed.card(deck, 'secret', 'diary');
  const { makeAccount, logIn } = await import('../support/fixtures.ts');
  const intruder = makeAccount('intruder');
  const context = await browser.newContext();
  const other = await context.newPage();
  await logIn(other.request, intruder);
  for (const path of [`/decks/${deck}`, `/decks/${deck}/edit`, `/cards/${card}`, `/decks/${deck}/export.csv`, `/study?deck=${deck}`]) {
    const res = await other.goto(path);
    expect(res?.status(), path).toBe(404);
    await expect(other.locator('body')).not.toContainText('diary');
  }
  const res = await other.request.post(`/cards/${card}/delete`, { maxRedirects: 0 });
  expect(res.status()).toBe(404);
  expect(sql('SELECT 1 FROM cards WHERE id = ?', card)).toHaveLength(1);
  await context.close();
  void page;
});
