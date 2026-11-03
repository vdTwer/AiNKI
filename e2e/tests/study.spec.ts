// Studying: the heart of the site. Answers never reload the page, keys
// work, undo puts the card back, and the end of a session is reached.
import { test, expect, sql, type Seed } from '../support/fixtures.ts';
import type { Page } from '@playwright/test';

const front = (page: Page) => page.locator('main .study .face.front');
const back = (page: Page) => page.locator('main .study .face.back');
const showAnswer = (page: Page) => page.locator('main .study .reveal a.show');
const answers = (page: Page) => page.locator('main .study form.answers');
const count = (page: Page) => page.locator('main .study-meta .count');

async function threeCards(seed: Seed) {
  const deck = await seed.deck('Languages::Spanish');
  await seed.cards(deck, [
    ['el perro', 'the dog'],
    ['el gato', 'the cat'],
    ['la casa', 'the house'],
  ]);
  return deck;
}

/** Mark the window: if the page reloads, the mark is gone. */
async function markWindow(page: Page) {
  await page.evaluate(() => ((window as unknown as { __kept: number }).__kept = 42));
}
async function expectSameWindow(page: Page) {
  expect(await page.evaluate(() => (window as unknown as { __kept?: number }).__kept)).toBe(42);
}

test('a whole session without a single page load', async ({ page, seed, account }) => {
  await threeCards(seed);
  await page.goto('/');
  await page.locator('main a.start').click();
  await expect(page).toHaveURL(/\/study/);
  await markWindow(page);

  const seen: string[] = [];
  for (let i = 1; i <= 3; i++) {
    await expect(count(page)).toHaveText(`${i} of 3`);
    await expect(back(page)).toBeHidden();
    await expect(answers(page)).toBeHidden();
    const text = (await front(page).innerText()).trim();
    expect(seen).not.toContain(text);
    seen.push(text);

    await showAnswer(page).click();
    await expect(back(page)).toBeVisible();
    await expect(answers(page)).toBeVisible();
    await answers(page).getByRole('button', { name: /good/i }).click();
  }
  await expect(page.locator('main .session-end')).toBeVisible();
  await expectSameWindow(page);
  expect(seen.sort()).toEqual(['el gato', 'el perro', 'la casa']);

  // Every answer is in the log, once.
  const [{ n }] = sql<{ n: number }>('SELECT COUNT(*) AS n FROM review_log WHERE user_id = ?', account.id);
  expect(n).toBe(3);

  // Home now says the day is done.
  await page.goto('/');
  await expect(page.locator('main')).not.toContainText('cards to study today');
});

test('keys: space shows the answer, 1 and 3 rate', async ({ page, seed, account }) => {
  await threeCards(seed);
  await page.goto('/study');
  await markWindow(page);

  await page.keyboard.press(' ');
  await expect(back(page)).toBeVisible();
  await page.keyboard.press('1'); // Again
  await expect(count(page)).toHaveText(/of 3/);
  await expect(back(page)).toBeHidden();

  await page.keyboard.press(' ');
  await expect(back(page)).toBeVisible();
  await page.keyboard.press(' '); // space again means Good
  await expect(back(page)).toBeHidden();
  await expectSameWindow(page);

  const ratings = sql<{ rating: number }>('SELECT rating FROM review_log WHERE user_id = ? ORDER BY id', account.id).map((r) => r.rating);
  expect(ratings).toEqual([1, 3]);
});

test('undo brings the last card back, answer showing', async ({ page, seed, account }) => {
  await threeCards(seed);
  await page.goto('/study');
  const first = (await front(page).innerText()).trim();
  await showAnswer(page).click();
  await answers(page).getByRole('button', { name: /good/i }).click();
  await expect(front(page)).not.toHaveText(first);

  await page.getByRole('button', { name: /undo last answer/i }).click();
  await expect(front(page)).toHaveText(first);
  await expect(back(page)).toBeVisible();
  expect(sql('SELECT 1 FROM review_log WHERE user_id = ?', account.id)).toHaveLength(0);
});

test('Again brings the card back later in the same session', async ({ page, seed, account }) => {
  const deck = await seed.deck('One');
  await seed.card(deck, 'only card', 'answer');
  await page.goto('/study');
  await showAnswer(page).click();
  await answers(page).getByRole('button', { name: /again/i }).click();
  // Its learning step is minutes away: nothing more right now, but the
  // page says when it comes back instead of claiming the day is done.
  await expect(page.locator('main')).toContainText(/minute|again|later|back/i);
  sql('UPDATE cards SET due = unixepoch() - 1 WHERE user_id = ?', account.id);
  await page.goto('/study');
  await expect(front(page)).toHaveText('only card');
});

test('answering a card twice from two tabs counts once', async ({ page, seed, context, account }) => {
  await threeCards(seed);
  await page.goto('/study');
  const other = await context.newPage();
  await other.goto('/study');
  const card = (await front(page).innerText()).trim();
  await expect(front(other)).toHaveText(card);

  await showAnswer(page).click();
  await answers(page).getByRole('button', { name: /good/i }).click();
  await expect(front(page)).not.toHaveText(card);
  await showAnswer(other).click();
  await answers(other).getByRole('button', { name: /good/i }).click();
  await expect(other.locator('main')).toBeVisible();
  expect(sql('SELECT 1 FROM review_log WHERE user_id = ?', account.id)).toHaveLength(1);
});

test('four buttons when the setting asks for them', async ({ page, seed, account }) => {
  await threeCards(seed);
  sql('UPDATE users SET four_buttons = 1 WHERE id = ?', account.id);
  await page.goto('/study');
  await showAnswer(page).click();
  const names = await answers(page).getByRole('button').allInnerTexts();
  expect(names.map((n) => n.split(/\s/)[0])).toEqual(['Again', 'Hard', 'Good', 'Easy']);
  await page.keyboard.press('4');
  await expect(back(page)).toBeHidden();
  expect(sql<{ rating: number }>('SELECT rating FROM review_log WHERE user_id = ?', account.id)[0].rating).toBe(4);
});

test.describe('without JavaScript', () => {
  test.use({ javaScriptEnabled: false });

  test('studying still works with plain links and forms', async ({ page, seed }) => {
    await threeCards(seed);
    await page.goto('/study');
    await expect(back(page)).toBeHidden();
    await showAnswer(page).click();
    await expect(back(page)).toBeVisible();
    await answers(page).getByRole('button', { name: /good/i }).click();
    await expect(count(page)).toHaveText('2 of 3');
    await page.getByRole('button', { name: /undo last answer/i }).click();
    await expect(back(page)).toBeVisible();
  });
});
