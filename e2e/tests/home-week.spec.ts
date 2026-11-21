// The desktop home's week looks ahead: tomorrow's cards, a forecast for
// the next seven days that counts the cards coming back, and a bar for
// today and each of the six days after it.
import { test, expect, sql, desktopOnly } from '../support/fixtures.ts';

const DAY = 86_400;

desktopOnly();

/** Make a card learned and due at a given time. */
function due(id: number, at: number) {
  sql('UPDATE cards SET state = 2, stability = 10, difficulty = 5, reps = 3, last_review = ?, due = ? WHERE id = ?', at - 10 * DAY, at, id);
}

test('home shows the next seven days, starting today', async ({ page, seed }) => {
  const deck = await seed.deck('Capitals');
  const [france, spain, peru, chad] = await seed.cards(deck, [
    ['France', 'Paris'],
    ['Spain', 'Madrid'],
    ['Peru', 'Lima'],
    ['Chad', "N'Djamena"],
  ]);
  const now = Math.floor(Date.now() / 1000);
  due(france, now - DAY);
  due(spain, now + DAY);
  due(peru, now + 3 * DAY);
  due(chad, now + 3 * DAY);

  await page.goto('/');
  const week = page.locator('.week-home');
  await expect(week.locator('h2')).toHaveText('Next seven days');
  await expect(week).not.toContainText('last seven days');

  const days = week.locator('.week-bars .day');
  await expect(days).toHaveCount(7);
  // Today comes first, in bold, and holds what the page says to study now.
  await expect(days.nth(0)).toHaveClass(/\btoday\b/);
  await expect(days.nth(0)).toHaveAttribute('title', 'Today: 1 card');
  await expect(days.nth(1)).toHaveAttribute('title', /^Tomorrow: \d+ cards?$/);
  // Later days are forecasts, and the two cards due in three days are
  // certainly there.
  await expect(days.nth(3)).toHaveAttribute('title', /^(Mon|Tue|Wed|Thu|Fri|Sat|Sun) \d+: about [2-9] cards$/);

  // The busiest day gets the full height; an empty day only a sliver.
  await expect(days.nth(3).locator('.bar')).toHaveAttribute('style', 'height: 100%');
  const text = week.locator('p').first();
  await expect(text).toContainText(/^(About )?\d+ cards? tomorrow\. Probably about \d+ in the next week/);
  // The forecast never drops what is already scheduled: four cards.
  const forecast = text.locator('.forecast');
  // Its tooltip gives the forecast, its bounds and why.
  await expect(forecast).toHaveAttribute('title', /^Forecast: \d+ cards\. Likely \d+ to \d+\. .*come back/);
  expect(Number((await forecast.getAttribute('title'))!.match(/\d+/)![0])).toBeGreaterThanOrEqual(4);
});

test('a quiet week says so and draws no bars', async ({ page, seed }) => {
  const deck = await seed.deck('Capitals');
  const id = await seed.card(deck, 'France', 'Paris');
  due(id, Math.floor(Date.now() / 1000) + 30 * DAY);
  await page.goto('/');
  const week = page.locator('.week-home');
  await expect(week).toContainText('Nothing comes due in the next week.');
  await expect(week.locator('.week-bars')).toHaveCount(0);
});

test('cards that come back within the week count too', async ({ page, seed }) => {
  const deck = await seed.deck('Capitals');
  const id = await seed.card(deck, 'France', 'Paris');
  // A young card, seen today and due tomorrow: after that review it
  // returns within days.
  const now = Math.floor(Date.now() / 1000);
  sql('UPDATE cards SET state = 2, stability = 1, difficulty = 5, reps = 2, last_review = ?, due = ? WHERE id = ?', now, now + DAY, id);
  await page.goto('/');
  const forecast = page.locator('.week-home .forecast');
  await expect(page.locator('.week-home p').first()).toContainText(/^1 card tomorrow\./);
  const n = Number((await forecast.getAttribute('title'))!.match(/\d+/)![0]);
  expect(n).toBeGreaterThanOrEqual(2);
});
