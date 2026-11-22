// Getting around: the top navigation everywhere, the quick find (a page,
// and with JavaScript a dialog) and the keys sheet.
import { test, expect, desktopOnly } from '../support/fixtures.ts';

const PLACES: [string, RegExp][] = [
  ['Study', /\/study$/],
  ['Add', /\/cards\/new$/],
  ['Stats', /\/stats$/],
  ['Settings', /\/settings$/],
  ['AiNKI', /\/$/],
];

test('every place is reachable from the navigation', async ({ page, seed }) => {
  await seed.deck('Something');
  await page.goto('/');
  const nav = page.getByRole('navigation', { name: 'Main' });
  for (const [name, url] of PLACES) {
    await nav.getByRole('link', { name, exact: true }).click();
    await expect(page).toHaveURL(url);
    await expect(nav.getByRole('link', { name, exact: true })).toHaveAttribute('aria-current', 'page');
  }
});

test('the menu reads Study, Add, Stats, Settings, Find', async ({ page }) => {
  await page.goto('/stats');
  const names = await page.locator('nav.top a:not(.brand)').allTextContents();
  expect(names).toEqual(['Study', 'Add', 'Stats', 'Settings', 'Find']);
});

test('quick find forgives typos and goes where you pick', async ({ page, seed }) => {
  const deck = await seed.deck('Languages::Spanish');
  const card = await seed.card(deck, 'el murciélago', 'the bat');
  await page.goto('/stats');

  await page.getByRole('navigation', { name: 'Main' }).getByRole('link', { name: 'Find' }).click();
  const sheet = page.locator('ainki-find dialog');
  await expect(sheet).toBeVisible();
  const input = sheet.locator('input[type=search]');
  await expect(input).toBeFocused();
  await input.fill('spansh');
  const rows = sheet.locator('#find-results a.row');
  await expect(rows.first()).toContainText('Spanish');
  await page.keyboard.press('Enter');
  await expect(page).toHaveURL(`/decks/${deck}`);

  // Slash opens it too; accents don't matter for cards.
  await page.keyboard.press('/');
  await expect(sheet).toBeVisible();
  await sheet.locator('input[type=search]').fill('murcielago');
  await expect(rows.first()).toContainText('murciélago');
  await rows.first().click();
  await expect(page).toHaveURL(new RegExp(`/cards/${card}`));
});

test.describe('without JavaScript', () => {
  test.use({ javaScriptEnabled: false });

  test('quick find is a page of its own', async ({ page, seed }) => {
    await seed.deck('Languages::Spanish');
    await page.goto('/');
    await page.getByRole('navigation', { name: 'Main' }).getByRole('link', { name: 'Find' }).click();
    await expect(page).toHaveURL('/find');
    await page.locator('main input[type=search]').fill('spansh');
    await page.locator('main').getByRole('button', { name: 'Find' }).click();
    await expect(page.locator('#find-results a.row').first()).toContainText('Spanish');
  });
});

test.describe('desktop', () => {
  desktopOnly();

  test('Ctrl+K, a typed letter, Escape and places', async ({ page }) => {
    await page.goto('/');
    const sheet = page.locator('ainki-find dialog');
    await page.keyboard.press('Control+k');
    await expect(sheet).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(sheet).toBeHidden();
    // A letter typed within milliseconds of Escape is sometimes lost in
    // headless Chromium (about 1 in 8); people don't type that fast.
    await page.waitForTimeout(300);

    // Any letter outside a field starts a search with it.
    await page.keyboard.press('s');
    await expect(sheet).toBeVisible();
    await expect(sheet.locator('input[type=search]')).toHaveValue('s');
    await sheet.locator('input[type=search]').fill('setings');
    await expect(sheet.locator('#find-results a.row').first()).toContainText('Settings');
    await page.keyboard.press('Enter');
    await expect(page).toHaveURL('/settings');
  });

  test('letters answer while studying instead of searching', async ({ page, seed }) => {
    const deck = await seed.deck('Keys');
    await seed.card(deck, 'front', 'back');
    await page.goto('/study');
    await page.keyboard.press('s');
    await expect(page.locator('ainki-find dialog')).toBeHidden();
    // Ctrl+K still works there.
    await page.keyboard.press('Control+k');
    await expect(page.locator('ainki-find dialog')).toBeVisible();
  });

  test('? shows the keys, once per page', async ({ page, seed }) => {
    const deck = await seed.deck('Keys');
    await seed.card(deck, 'front', 'back');
    for (const path of ['/', '/study', `/decks/${deck}`, '/cards/new']) {
      await page.goto(path);
      await expect(page.locator('.keys-hint')).toHaveCount(1);
    }
    // While a card is up, the hint keeps out of sight; the key still works.
    await page.goto('/study');
    await expect(page.locator('.keys-hint')).toBeHidden();
    await page.keyboard.press('?');
    const sheet = page.locator('#keys');
    await expect(sheet).toBeVisible();
    await expect(sheet).toContainText(/space/i);
    await page.keyboard.press('Escape');
    await expect(sheet).toBeHidden();
    // Elsewhere the hint opens it with a click, no script needed.
    await page.goto('/');
    await page.locator('.keys-hint button').click();
    await expect(sheet).toBeVisible();
  });
});

test('logged-out visitors are sent to the login and come back after', async ({ browser, page: loggedIn, seed }) => {
  const deck = await seed.deck('Deep link');
  const context = await browser.newContext();
  const page = await context.newPage();
  await page.goto(`/decks/${deck}`);
  await expect(page).toHaveURL(/\/login/);
  // The same account logs in through the form.
  const { sql } = await import('../support/fixtures.ts');
  const [{ username, discriminator }] = sql<{ username: string; discriminator: number }>(
    'SELECT username, discriminator FROM users u JOIN decks d ON d.user_id = u.id WHERE d.id = ?',
    deck,
  );
  await page.locator('input[name=username]').fill(`${username}#${String(discriminator).padStart(4, '0')}`);
  await page.locator('input[name=password]').fill('correct horse battery');
  await page.getByRole('button', { name: 'Log in' }).click();
  await expect(page).toHaveURL(`/decks/${deck}`);
  await context.close();
  void loggedIn;
});
