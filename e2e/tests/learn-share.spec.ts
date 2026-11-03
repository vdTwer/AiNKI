// Lessons, and decks shared between accounts.
import { test, expect, sql, makeAccount, logIn } from '../support/fixtures.ts';
import { flash } from '../support/ui.ts';

test('a lesson can be read and added as a deck', async ({ page, account }) => {
  await page.goto('/learn');
  const lessons = page.locator('main a.row');
  expect(await lessons.count()).toBeGreaterThanOrEqual(18);
  await lessons.first().click();
  await expect(page).toHaveURL(/\/learn\/[\w-]+$/);
  const title = (await page.locator('main h1').innerText()).trim();
  await page.getByRole('button', { name: 'Add as a deck' }).click();
  const [deck] = sql<{ id: number; n: number }>(
    'SELECT d.id, (SELECT COUNT(*) FROM cards c WHERE c.deck_id = d.id) AS n FROM decks d WHERE d.user_id = ?',
    account.id,
  );
  expect(deck.n).toBeGreaterThan(0);
  // Once added, the lesson links to its deck instead.
  await page.goBack();
  await page.reload();
  await expect(page.locator('main')).toContainText('This lesson is in your decks.');
  await expect(page.locator('main h1')).toHaveText(title);
});

test('every lesson opens and links to lessons that exist', async ({ page }) => {
  await page.goto('/learn');
  const hrefs = await page.locator('main a.row').evaluateAll((as) => as.map((a) => a.getAttribute('href')!));
  for (const href of hrefs) {
    const res = await page.goto(href);
    expect(res?.status(), href).toBe(200);
    for (const link of await page.locator('main a[href^="/learn/"]').evaluateAll((as) => as.map((a) => a.getAttribute('href')!))) {
      expect(hrefs, `${href} links to ${link}`).toContain(link.replace(/#.*$/, ''));
    }
  }
});

test('a shared deck arrives as an independent copy', async ({ page, seed, browser }) => {
  const deck = await seed.deck('Languages::Spanish');
  await seed.cards(deck, [['el perro', 'the dog'], ['el gato', 'the cat']]);
  const friend = makeAccount('friend');

  await page.goto(`/decks/${deck}/edit#share`);
  const form = page.locator(`form[action="/decks/${deck}/share"]`);
  await form.locator('input[name=user]').fill(friend.handle);
  await form.getByRole('button', { name: 'Share' }).click();
  await expect(flash(page)).toContainText(`Shared with ${friend.handle}`);

  const ctx = await browser.newContext();
  const other = await ctx.newPage();
  await logIn(other.request, friend);
  await other.goto('/');
  await expect(other.locator('main')).toContainText('Spanish');
  await other.getByRole('button', { name: 'Accept' }).click();
  await expect(flash(other)).toHaveText('Shared deck added to your decks.');
  const [copy] = sql<{ id: number; name: string }>('SELECT id, name FROM decks WHERE user_id = ?', friend.id);
  expect(copy.name).toBe('Languages::Spanish');
  expect(sql('SELECT front FROM cards WHERE deck_id = ? ORDER BY front', copy.id)).toEqual([{ front: 'el gato' }, { front: 'el perro' }]);

  // Changes on one side don't reach the other.
  sql("UPDATE cards SET back = 'changed' WHERE deck_id = ?", deck);
  expect(sql<{ back: string }>('SELECT back FROM cards WHERE deck_id = ?', copy.id).map((c) => c.back).sort()).toEqual(['the cat', 'the dog']);
  await ctx.close();
});

test('sharing with a name that does not exist says so', async ({ page, seed, allowErrors }) => {
  allowErrors.push(/status of 4\d\d/);
  const deck = await seed.deck('Lonely');
  await page.goto(`/decks/${deck}/edit#share`);
  const form = page.locator(`form[action="/decks/${deck}/share"]`);
  await form.locator('input[name=user]').fill('nobody#0001');
  await form.getByRole('button', { name: 'Share' }).click();
  await expect(page.locator('main')).toContainText(/no (such )?user|not found|unknown/i);
  expect(sql('SELECT 1 FROM deck_shares WHERE deck_id = ?', deck)).toHaveLength(0);
});
