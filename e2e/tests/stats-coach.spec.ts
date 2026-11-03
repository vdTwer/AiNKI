// Stats with real history, and the coach: a note on Home and Stats, the
// cards needing a look, "Keep as is" and "Not now".
import { test, expect, sql, isCalm, desktopOnly, type Seed } from '../support/fixtures.ts';
import { flash } from '../support/ui.ts';

const DAY = 86_400;

/** Twelve weeks of reviews: mostly remembered, a few forgotten. */
async function history(seed: Seed, userId: number) {
  const deck = await seed.deck('Capitals');
  const ids = await seed.cards(deck, [
    ['France', 'Paris'],
    ['Spain', 'Madrid'],
    ['Peru', 'Lima'],
    ['Chad', "N'Djamena"],
  ]);
  const now = Math.floor(Date.now() / 1000);
  for (const [i, id] of ids.entries()) {
    for (let d = 80; d > 0; d -= 4 + i) {
      sql(
        `INSERT INTO review_log (user_id, card_id, deck_id, rating, reviewed_at, state_before, elapsed_days,
           scheduled_secs, stability, difficulty, duration_ms, source)
         VALUES (?, ?, ?, ?, ?, 2, 4, 345600, 10, 5, 4200, 'web')`,
        userId, id, deck, d % 3 === 0 ? 1 : 3, now - d * DAY,
      );
    }
    sql('UPDATE cards SET state = 2, stability = 12, difficulty = 5, reps = 10, last_review = ?, due = ? WHERE id = ?', now - 2 * DAY, now + (i + 1) * DAY, id);
  }
  return deck;
}

test('stats show charts on the desktop and the exact numbers everywhere', async ({ page, seed, account }) => {
  const deck = await history(seed, account.id);
  await page.goto('/stats');
  await expect(page.locator('main h1, main h2').first()).toBeVisible();
  if (await isCalm(page)) {
    // Every chart is drawn from its numbers. A chart that throws is
    // caught by desk.js and only marked data-failed, so look for that.
    const charts = page.locator('.desk-stats .chart[data-chart]');
    expect(await charts.count()).toBeGreaterThanOrEqual(4);
    for (const chart of await charts.all()) {
      const kind = (await chart.getAttribute('data-chart')) ?? '';
      await expect(chart, kind).toHaveAttribute('data-drawn', '');
      await expect(chart, kind).not.toHaveAttribute('data-failed', '');
      expect(await chart.locator('*').count(), `${kind} has something in it`).toBeGreaterThan(0);
    }
    // Exact numbers sit behind "I love numbers".
    const numbers = page.locator('main .numbers');
    await expect(numbers).toBeHidden();
    await page.locator('label.numbers-toggle').click();
    await expect(numbers).toBeVisible();
  } else {
    await expect(page.locator('main .numbers')).toBeVisible();
  }
  await expect(page.locator('main')).toContainText('Capitals');

  // One deck's stats.
  await page.goto(`/stats?deck=${deck}`);
  await expect(page.locator('main')).toContainText('Capitals');
  await expect(page.locator('main')).not.toContainText('::');
});

test('stats for a brand-new account have nothing to divide by zero', async ({ page }) => {
  await page.goto('/stats');
  await expect(page.locator('main')).not.toContainText(/NaN|undefined|Infinity|null/);
});

test('the coach points at a leech; Keep as is and Not now quiet it', async ({ page, seed, account }) => {
  const deck = await seed.deck('Hard facts');
  const leech = await seed.card(deck, 'Capital of Burkina Faso', 'Ouagadougou');
  await seed.card(deck, 'Hund', 'dog');
  const now = Math.floor(Date.now() / 1000);
  sql('UPDATE cards SET state = 2, stability = 3, difficulty = 8, lapses = 6, reps = 12, last_review = ?, due = ? WHERE id = ?', now - DAY, now + 2 * DAY, leech);

  await page.goto('/');
  if (await isCalm(page)) {
    // On the desktop the note waits behind a ! beside the heading.
    const mark = page.getByRole('button', { name: 'A note from the coach' });
    await mark.hover();
    const note = page.locator('.coach-note');
    await expect(note).toBeVisible();
    await expect(note).toContainText('1 card is forgotten again and again');
    await note.getByRole('link', { name: 'Have a look' }).click();
  } else {
    await page.goto('/attention');
  }
  await expect(page).toHaveURL('/attention');
  await expect(page.locator('main a.row')).toHaveCount(1);
  await page.locator(`main a.row[href="/cards/${leech}"]`).click();
  await page.getByRole('button', { name: 'Keep as is' }).first().click();
  await expect(flash(page)).toHaveText('Kept as is.');
  await page.goto('/attention');
  await expect(page.locator('main a.row')).toHaveCount(0);
  expect(sql('SELECT kind FROM coach_dismissals WHERE card_id = ?', leech)).toEqual([{ kind: 'leech' }]);

  // A second leech brings the note back; Not now hides it for three days.
  const second = await seed.card(deck, 'Capital of Kyrgyzstan', 'Bishkek');
  sql('UPDATE cards SET state = 2, stability = 3, difficulty = 8, lapses = 7, reps = 12, last_review = ?, due = ? WHERE id = ?', now - DAY, now + 2 * DAY, second);
  await page.goto('/stats');
  if (await isCalm(page)) {
    await page.locator('.tip[data-rule=leeches]').first().getByRole('button', { name: 'Not now' }).click();
    await expect(page).toHaveURL('/stats');
    await expect(page.locator('.tip[data-rule=leeches]')).toHaveCount(0);
    const [{ until }] = sql<{ until: number }>('SELECT until FROM coach_snoozes WHERE user_id = ? AND rule = ?', account.id, 'leeches');
    expect(until - now).toBeGreaterThan(2.9 * DAY);
    await page.goto('/');
    await expect(page.locator('.tip[data-rule=leeches]')).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'A note from the coach' })).toHaveCount(0);
  }
});

test('the coach says nothing to a new learner', async ({ page, seed }) => {
  const deck = await seed.deck('Fresh');
  await seed.card(deck, 'a', 'b');
  for (const path of ['/', '/stats']) {
    await page.goto(path);
    await expect(page.locator('.tip')).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'A note from the coach' })).toHaveCount(0);
  }
});

test.describe('known bugs', () => {
  // Stats never shows flash messages, so "Not now" there is silent (Home
  // says "The coach will keep that to itself for three days.").
  desktopOnly();
  test.fail();
  test('Not now on Stats confirms it', async ({ page, seed }) => {
    const deck = await seed.deck('Hard facts');
    const leech = await seed.card(deck, 'Capital of Burkina Faso', 'Ouagadougou');
    const now = Math.floor(Date.now() / 1000);
    sql('UPDATE cards SET state = 2, stability = 3, difficulty = 8, lapses = 6, reps = 12, last_review = ?, due = ? WHERE id = ?', now - DAY, now + 2 * DAY, leech);
    await page.goto('/stats');
    await page.locator('.tip[data-rule=leeches]').first().getByRole('button', { name: 'Not now' }).click();
    await expect(flash(page)).toHaveText('The coach will keep that to itself for three days.', { timeout: 2000 });
  });
});
