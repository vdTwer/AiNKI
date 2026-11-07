// Pausing and archiving decks: their cards rest, the deck says so, and
// resuming or restoring brings it back as it was.
import { test, expect, sql } from '../support/fixtures.ts';
import { flash } from '../support/ui.ts';

test('a paused deck stays listed, is not studied and resumes as it was', async ({ page, seed }) => {
  const spanish = await seed.deck('Spanish');
  const french = await seed.deck('French');
  const [, kept] = await seed.cards(spanish, [['hablar', 'to speak'], ['comer', 'to eat']]);
  await seed.cards(french, [['parler', 'to speak']]);
  // Suspended by hand before the pause: resuming must leave it so.
  await page.request.post(`/cards/${kept}/suspend`);

  await page.goto(`/decks/${spanish}/edit`);
  await page.getByRole('button', { name: 'Pause deck' }).click();
  await expect(page).toHaveURL(new RegExp(`/decks/${spanish}$`));
  await expect(flash(page)).toHaveText('Deck paused.');
  await expect(page.locator('main')).toContainText('Paused. Its cards wait');

  await page.goto('/');
  const row = page.locator('ul.decks li', { hasText: 'Spanish' });
  await expect(row).toContainText('Paused');
  await expect(page.locator('h1')).toContainText('1 card');

  await page.goto('/study');
  await expect(page.locator('main')).toContainText('parler');

  await page.goto(`/decks/${spanish}`);
  await page.getByRole('button', { name: 'Resume deck' }).click();
  await expect(flash(page)).toHaveText('Deck back in study.');
  expect(sql('SELECT front, suspended FROM cards WHERE deck_id = ? ORDER BY front', spanish)).toEqual([
    { front: 'comer', suspended: 1 },
    { front: 'hablar', suspended: 0 },
  ]);
});

test('an archived deck leaves the list and comes back on restore', async ({ page, seed }) => {
  const old = await seed.deck('Old exam');
  await seed.deck('Current');
  await seed.cards(old, [['Q', 'A']]);

  await page.goto(`/decks/${old}/edit`);
  await page.getByRole('button', { name: 'Archive deck' }).click();
  await expect(page).toHaveURL(/\/$/);
  await expect(flash(page)).toContainText('Deck archived.');
  await expect(page.locator('ul.decks')).not.toContainText('Old exam');

  // Not offered when adding a card.
  await page.goto('/cards/new');
  await expect(page.locator('select[name=deck] option', { hasText: 'Old exam' })).toHaveCount(0);

  await page.goto('/');
  await page.getByRole('link', { name: '1 archived deck' }).click();
  await expect(page).toHaveURL(/\/decks\/archived$/);
  await page.getByRole('link', { name: /Old exam/ }).click();
  await expect(page.locator('main')).toContainText('Archived.');
  await page.getByRole('button', { name: 'Restore deck' }).click();
  await expect(flash(page)).toHaveText('Deck back in study.');

  await page.goto('/');
  await expect(page.locator('ul.decks')).toContainText('Old exam');
  await expect(page.getByRole('link', { name: /archived deck/ })).toHaveCount(0);
  expect(sql('SELECT status FROM decks WHERE id = ?', old)).toEqual([{ status: 'active' }]);
});
