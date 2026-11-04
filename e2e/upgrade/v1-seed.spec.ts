// Runs against the v1 server (main): an account used the way people use
// it today, through v1's own website.
import { test, expect } from '@playwright/test';
import { makeAccount, logIn, sql } from '../support/fixtures.ts';
import { png } from '../support/files.ts';
import { save, storageFile } from './state.ts';

const COUNTS = ['users', 'decks', 'cards', 'card_items', 'review_log', 'item_review_log', 'presets', 'media', 'api_tokens', 'deck_shares', 'sessions'];

test('v1 writes a well-used account', async ({ page }) => {
  const user = makeAccount('veteran');
  const friend = makeAccount('friend');
  await logIn(page.request, user);

  // Decks and cards through the forms.
  const deckRes = await page.request.post('/decks/new', { form: { name: 'Languages::Spanish', description: 'from v1' }, maxRedirects: 0 });
  expect(deckRes.status()).toBe(303);
  const [{ id: deckId }] = sql<{ id: number }>('SELECT id FROM decks WHERE user_id = ?', user.id);
  for (const [front, back] of [['el perro', 'the dog'], ['el gato', 'the cat'], ['la casa', 'the house'], ['el sol', 'the sun']]) {
    expect((await page.request.post('/cards/new', { multipart: { deck: String(deckId), front, back }, maxRedirects: 0 })).status()).toBe(303);
  }
  await page.request.post('/cards/new', { multipart: { deck: String(deckId), front: 'Three verbs', back: '- ser\n- estar\n- ir', multi: 'list' }, maxRedirects: 0 });
  await page.request.post('/cards/new', {
    multipart: { deck: String(deckId), front: 'A flag', back: 'Spain', front_image: { name: 'es.png', mimeType: 'image/png', buffer: png(30, 20) } },
    maxRedirects: 0,
  });
  const [{ id: listCardId }] = sql<{ id: number }>("SELECT id FROM cards WHERE user_id = ? AND front = 'Three verbs'", user.id);
  const [{ id: imageCardId, front_image: imageId }] = sql<{ id: number; front_image: string }>(
    "SELECT id, front_image FROM cards WHERE user_id = ? AND front = 'A flag'",
    user.id,
  );
  expect(imageId).toBeTruthy();

  // Study in the real v1 study view: answer every card once.
  await page.goto('/study');
  const answered = () => sql<{ n: number }>('SELECT COUNT(*) AS n FROM review_log WHERE user_id = ?', user.id)[0].n;
  for (let i = 0; i < 12 && answered() < 5; i++) {
    const show = page.locator('main a.show').first();
    await expect(show).toBeVisible();
    await show.click();
    const list = page.locator('main form.list-answer');
    if (await list.isVisible()) {
      await list.locator('label:has(input[name=recalled])').first().click();
      await list.getByRole('button', { name: /done/i }).click();
    } else {
      await page.locator('main form.answers').getByRole('button', { name: i % 3 ? /good/i : /again/i }).click();
    }
    await page.waitForLoadState('networkidle');
  }
  const reviews = answered();
  expect(reviews).toBeGreaterThanOrEqual(5);
  // Some history from earlier weeks too.
  const now = Math.floor(Date.now() / 1000);
  sql(
    `INSERT INTO review_log (user_id, card_id, deck_id, rating, reviewed_at, state_before, elapsed_days, scheduled_secs, source)
     SELECT user_id, id, deck_id, 3, ? , 2, 7, 604800, 'web' FROM cards WHERE user_id = ? AND multi IS NULL`,
    now - 20 * 86_400,
    user.id,
  );

  // An agent token, made on v1's access page.
  await page.goto('/access');
  for (const d of await page.locator('main details:not([open])').all()) await d.locator('summary').click().catch(() => {});
  const form = page.locator('form[action="/access/new"]');
  await form.locator('input[name=name]').fill('Claude');
  await form.getByRole('button').last().click();
  const token = (await page.locator('main').innerText()).match(/ainki_[0-9a-f]{20,}/)?.[0];
  expect(token).toBeTruthy();

  // A deck offered to a friend, not yet accepted.
  const share = await page.request.post(`/decks/${deckId}/share`, { form: { user: friend.handle }, maxRedirects: 0 });
  expect(share.status()).toBe(303);

  // Settings from v1.
  sql("UPDATE users SET four_buttons = 1, font_scale = 110, desktop_look = 'calm', created_at = ? WHERE id = ?", now - 90 * 86_400, user.id);

  const counts = Object.fromEntries(COUNTS.map((t) => [t, sql<{ n: number }>(`SELECT COUNT(*) AS n FROM ${t}`)[0].n]));
  const [memory] = sql<{ stability: number; difficulty: number; due: number; reps: number }>(
    "SELECT stability, difficulty, due, reps FROM cards WHERE user_id = ? AND front = 'el gato'",
    user.id,
  );
  await page.context().storageState({ path: storageFile });
  save({ user, friend, deckId, listCardId, imageCardId, imageId, token: token!, counts, memory, storageState: storageFile });
});
