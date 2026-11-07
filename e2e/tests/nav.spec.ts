// Getting around: the top navigation everywhere, and on the desktop the
// dot that opens into icons, the quick find sheet and the keys sheet.
import { test, expect, desktopOnly, isCalm } from '../support/fixtures.ts';

const PLACES: [string, RegExp][] = [
  ['Study', /\/study$/],
  ['Add', /\/cards\/new$/],
  ['Stats', /\/stats$/],
  ['Learn', /\/learn$/],
  ['Settings', /\/settings$/],
  ['Home', /\/$/],
];

test('every place is reachable from the navigation', async ({ page, seed }) => {
  await seed.deck('Something');
  await page.goto('/');
  const nav = page.getByRole('navigation', { name: 'Main' });
  const calm = await isCalm(page);
  for (const [name, url] of PLACES) {
    // On the desktop the places open out of the dot when the pointer is near.
    if (calm) await nav.hover();
    const link = nav.getByRole('link', { name, exact: true });
    if (!(await link.isVisible()) && !calm) {
      // Phones keep Learn under Settings: the simple top bar never changes.
      test.info().annotations.push({ type: 'not in the bar', description: name });
      continue;
    }
    await link.click();
    await expect(page).toHaveURL(url);
    await expect(nav.getByRole('link', { name, exact: true })).toHaveAttribute('aria-current', 'page');
  }
});

test.describe('desktop', () => {
  desktopOnly();

  test('the dot itself opens into Home', async ({ page }) => {
    await page.goto('/stats');
    // The resting dot sits 31px in from the nav's right edge, mid-height.
    const box = await page.locator('nav.top').boundingBox();
    if (!box) throw new Error('no nav');
    const x = box.x + box.width - 31;
    const y = box.y + box.height / 2;
    await page.mouse.move(x, y);
    const home = page.locator('nav.top a[href="/"][data-label="Home"]');
    await expect(home.locator('.tile')).toHaveCSS('opacity', '1');
    const under = await page.evaluate(
      ([px, py]) => document.elementFromPoint(px, py)?.closest('a, button')?.getAttribute('data-label') ?? null,
      [x, y],
    );
    expect(under).toBe('Home');
  });

  test('quick find forgives typos and goes where you pick', async ({ page, seed }) => {
    const deck = await seed.deck('Languages::Spanish');
    const card = await seed.card(deck, 'el murciélago', 'the bat');
    await page.goto('/stats');

    // The dot opens into icons near the pointer; the first is Find.
    await page.getByRole('navigation', { name: 'Main' }).hover();
    await page.locator('nav.top .find-dot').click();
    const sheet = page.locator('dialog.find-sheet');
    await expect(sheet).toBeVisible();
    const input = sheet.locator('input[type=search]');
    await expect(input).toBeFocused();
    await input.fill('spansh');
    const rows = sheet.locator('.find-results .find-row');
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

  test('Ctrl+K, a typed letter, Escape and places', async ({ page }) => {
    await page.goto('/');
    const sheet = page.locator('dialog.find-sheet');
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
    await expect(sheet.locator('.find-row').first()).toContainText('Settings');
    await page.keyboard.press('Enter');
    await expect(page).toHaveURL('/settings');
  });

  test('letters answer while studying instead of searching', async ({ page, seed }) => {
    const deck = await seed.deck('Keys');
    await seed.card(deck, 'front', 'back');
    await page.goto('/study');
    await page.keyboard.press('s');
    await expect(page.locator('dialog.find-sheet')).toHaveCount(0);
    // Ctrl+K still works there.
    await page.keyboard.press('Control+k');
    await expect(page.locator('dialog.find-sheet')).toBeVisible();
  });

  test('? shows the keys, once per page', async ({ page, seed }) => {
    const deck = await seed.deck('Keys');
    await seed.card(deck, 'front', 'back');
    for (const path of ['/', '/study', `/decks/${deck}`, '/cards/new']) {
      await page.goto(path);
      await expect(page.locator('.keys-button')).toHaveCount(1);
    }
    await page.goto('/study');
    await page.keyboard.press('?');
    const sheet = page.locator('dialog[open]');
    await expect(sheet).toBeVisible();
    await expect(sheet).toContainText(/space/i);
    await page.keyboard.press('Escape');
    await expect(page.locator('dialog[open]')).toHaveCount(0);
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
