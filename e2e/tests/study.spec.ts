// Studying: the heart of the site. Answers never reload the page, keys
// work, undo puts the card back, and the end of a session is reached.
import { test, expect, sql, type Seed } from '../support/fixtures.ts';
import type { Page } from '@playwright/test';

const front = (page: Page) => page.locator('main .study .face.front');
const back = (page: Page) => page.locator('main .study .face.back');
const showAnswer = (page: Page) => page.locator('main .study .reveal a.show');
const answers = (page: Page) => page.locator('main .study form.answers');
const count = (page: Page) => page.locator('main .study-meta .count .of');

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

test('Again brings the card back in the same session', async ({ page, seed }) => {
  const deck = await seed.deck('One');
  await seed.card(deck, 'only card', 'answer');
  await page.goto('/study');
  await showAnswer(page).click();
  await answers(page).getByRole('button', { name: /again/i }).click();
  // Nothing else is due, so its short learning step is not waited out:
  // the card comes straight back, now marked as learning.
  await expect(page.locator('main')).toContainText('Learning');
  await expect(front(page)).toHaveText('only card');
  await expect(showAnswer(page)).toBeVisible();
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

/** Pretend the tab went to the background, or came back. */
async function setHidden(page: Page, hidden: boolean) {
  await page.evaluate((h) => {
    Object.defineProperty(document, 'hidden', { value: h, configurable: true });
    document.dispatchEvent(new Event('visibilitychange'));
  }, hidden);
}

type Timing = { front: string; think_ms: number | null; answer_ms: number | null; away_ms: number | null; paused_ms: number | null; input: string | null };
const timings = (user: number) =>
  sql<Timing>(
    `SELECT c.front, r.think_ms, r.answer_ms, r.away_ms, r.paused_ms, r.input FROM review_log r
     JOIN cards c ON c.id = r.card_id WHERE r.user_id = ? ORDER BY r.id`,
    user,
  );
const near = (got: number | null, want: number) => expect(Math.abs((got ?? -1e9) - want)).toBeLessThanOrEqual(50);

test('each answer carries its timing, and time in another tab is kept apart', async ({ page, seed, account }) => {
  const deck = await seed.deck('One');
  await seed.cards(deck, [['el perro', 'the dog'], ['el gato', 'the cat']]);
  // Time stands still and moves only when the test says so.
  const start = new Date('2026-11-20T10:00:00Z');
  await page.clock.install({ time: start });
  await page.clock.pauseAt(start);
  await page.goto('/study');
  await expect(front(page)).toBeVisible();
  const first = (await front(page).innerText()).trim();

  // By keys: 2 s to recall, then 0.5 s on the back, 3 s away, 0.3 s more.
  await page.clock.runFor(2000);
  await page.keyboard.press(' ');
  await expect(back(page)).toBeVisible();
  await page.clock.runFor(500);
  await setHidden(page, true);
  await page.clock.runFor(3000);
  await setHidden(page, false);
  await page.clock.runFor(300);
  await page.keyboard.press('3');
  await expect(front(page)).not.toHaveText(first);

  // By mouse: 1.2 s, then 0.7 s.
  await page.clock.runFor(1200);
  await showAnswer(page).click();
  await page.clock.runFor(700);
  await answers(page).getByRole('button', { name: /good/i }).click();
  // The timing lands just after the review row itself.
  await expect.poll(() => timings(account.id).filter((t) => t.input !== null).length).toBe(2);

  const [a, b] = timings(account.id);
  near(a.think_ms, 2000);
  near(a.answer_ms, 800);
  near(a.away_ms, 3000);
  expect(a.input).toBe('key');
  near(b.think_ms, 1200);
  near(b.answer_ms, 700);
  expect(b.away_ms).toBe(0);
  expect(b.input).toMatch(/^(mouse|touch)$/);
});

test('a pause covers the card, and its time is kept apart', async ({ page, seed, account }) => {
  const deck = await seed.deck('One');
  await seed.cards(deck, [['el perro', 'the dog']]);
  const start = new Date('2026-11-20T10:00:00Z');
  await page.clock.install({ time: start });
  await page.clock.pauseAt(start);
  await page.goto('/study');
  await expect(front(page)).toBeVisible();
  const pauseButton = page.getByRole('button', { name: 'Pause' });

  await page.clock.runFor(1000);
  await pauseButton.click();
  await expect(page.locator('main .pause-cover')).toBeVisible();
  await expect(front(page)).toBeHidden();
  // Keys don't reach the card while it waits.
  await page.keyboard.press('3');
  await page.keyboard.press(' ');
  await expect(page.locator('main .pause-cover')).toBeHidden();
  // Space resumed it; the answer is still hidden.
  await expect(back(page)).toBeHidden();
  await expect(front(page)).toBeVisible();

  // The same with the key, a minute long, after 0.5 s more of thinking.
  await page.clock.runFor(500);
  await page.keyboard.press('p');
  await expect(page.locator('main .pause-cover')).toBeVisible();
  await page.clock.runFor(60_000);
  await page.getByRole('button', { name: 'Resume' }).click();
  await page.clock.runFor(300);
  await page.keyboard.press(' ');
  await expect(back(page)).toBeVisible();
  await page.clock.runFor(400);
  await page.keyboard.press('3');

  await expect.poll(() => timings(account.id).filter((t) => t.input !== null).length).toBe(1);
  const [t] = timings(account.id);
  near(t.think_ms, 1800);
  near(t.answer_ms, 400);
  near(t.paused_ms, 60_000);
  expect(t.away_ms).toBe(0);
});

test('the count swaps to cards left on click, and stays that way', async ({ page, seed }) => {
  await threeCards(seed);
  await page.goto('/study');
  const foot = page.locator('main .study-meta .count');
  await expect(foot).toHaveText('1 of 3', { useInnerText: true });
  await foot.click();
  await expect(foot).toHaveText('3 cards left', { useInnerText: true });
  // Kept across answers (no reload) and across page loads.
  await showAnswer(page).click();
  await answers(page).getByRole('button', { name: /good/i }).click();
  await expect(foot).toHaveText('2 cards left', { useInnerText: true });
  await page.reload();
  await expect(foot).toHaveText('2 cards left', { useInnerText: true });
  // Space still shows the answer, not the count again.
  await page.keyboard.press('Space');
  await expect(back(page)).toBeVisible();
  await foot.click();
  await expect(foot).toHaveText('2 of 3', { useInnerText: true });
});
