// Merged decks: a new deck made of copies; the decks it is made from stay
// as they are.
import { test, expect, sql, REFUSED } from '../support/fixtures.ts';
import { flash } from '../support/ui.ts';

test('merging two decks makes a new deck of copies', async ({ page, seed, account }) => {
  const spanish = await seed.deck('Languages::Spanish');
  const french = await seed.deck('French');
  await seed.deck('Untouched');
  await seed.cards(spanish, [['el perro', 'the dog'], ['el gato', 'the cat']]);
  await seed.cards(french, [['le chien', 'the dog'], ['el gato', 'the cat']]);
  sql('UPDATE cards SET reps = 3, state = 2 WHERE deck_id = ?', spanish);

  // From the deck page, the deck is picked already.
  await page.goto(`/decks/merge?with=${spanish}`);
  const form = page.locator('form[action="/decks/merge"]');
  await expect(form.locator(`input[name=decks][value="${spanish}"]`)).toBeChecked();
  await expect(form).toContainText('Languages › Spanish');
  await expect(form).not.toContainText('::');
  await form.locator(`input[name=decks][value="${french}"]`).check();
  await expect(form.locator(`input[name=decks][value="${french}"]`)).toBeChecked();
  await form.locator('input[name=name]').fill('All languages');
  await expect(form.locator('input[name=keep_progress]')).not.toBeChecked();
  await form.getByRole('button', { name: 'Merge decks' }).click();

  await expect(flash(page)).toHaveText('Decks merged: 3 cards copied. 1 identical card left out, already in it once.');
  await expect(page.locator('main h1')).toHaveText('All languages');
  const [merged] = sql<{ id: number }>("SELECT id FROM decks WHERE user_id = ? AND name = 'All languages'", account.id);
  // Decks in the order of the list (by name), each in its own order.
  expect(sql('SELECT front, reps FROM cards WHERE deck_id = ? ORDER BY position', merged.id)).toEqual([
    { front: 'le chien', reps: 0 },
    { front: 'el gato', reps: 0 },
    { front: 'el perro', reps: 0 },
  ]);
  // The originals are unchanged.
  expect(sql('SELECT COUNT(*) AS n, SUM(reps) AS reps FROM cards WHERE deck_id = ?', spanish)).toEqual([{ n: 2, reps: 6 }]);
  expect(sql('SELECT COUNT(*) AS n FROM cards WHERE deck_id = ?', french)).toEqual([{ n: 2 }]);
});

test('keeping progress copies the schedule', async ({ page, seed, account }) => {
  const a = await seed.deck('A');
  const b = await seed.deck('B');
  await seed.cards(a, [['one', '1']]);
  await seed.cards(b, [['two', '2']]);
  sql('UPDATE cards SET reps = 3, state = 2 WHERE deck_id = ?', a);

  await page.goto('/decks/merge');
  const form = page.locator('form[action="/decks/merge"]');
  await form.locator(`input[name=decks][value="${a}"]`).check();
  await form.locator(`input[name=decks][value="${b}"]`).check();
  await form.locator('input[name=name]').fill('AB');
  await form.locator('input[name=keep_progress]').check();
  await form.getByRole('button', { name: 'Merge decks' }).click();
  await expect(flash(page)).toHaveText('Decks merged: 2 cards copied.');
  const [merged] = sql<{ id: number }>("SELECT id FROM decks WHERE user_id = ? AND name = 'AB'", account.id);
  expect(sql('SELECT front, reps, state FROM cards WHERE deck_id = ? ORDER BY position', merged.id)).toEqual([
    { front: 'one', reps: 3, state: 2 },
    { front: 'two', reps: 0, state: 0 },
  ]);
});

test('one deck is not enough, and what was typed stays', async ({ page, seed, allowErrors }) => {
  allowErrors.push(REFUSED);
  const a = await seed.deck('Only');
  await seed.deck('Other');
  await page.goto(`/decks/merge?with=${a}`);
  const form = page.locator('form[action="/decks/merge"]');
  await form.locator('input[name=name]').fill('Lonely');
  await form.getByRole('button', { name: 'Merge decks' }).click();
  await expect(page.locator('main p.notice[role=alert]')).toHaveText('Pick at least two decks to merge');
  await expect(page.locator('input[name=name]')).toHaveValue('Lonely');
  await expect(page.locator(`input[name=decks][value="${a}"]`)).toBeChecked();
});

test('the new-deck page links to merging', async ({ page }) => {
  await page.goto('/decks/new');
  await page.getByRole('link', { name: 'merge decks you have' }).click();
  await expect(page).toHaveURL(/\/decks\/merge$/);
  await expect(page.locator('main')).toContainText('Merging needs at least two decks.');
});
