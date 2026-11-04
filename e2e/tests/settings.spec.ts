// Settings and the study options they turn on: the look, four buttons,
// quiet hours, timed and typed cards, the sure check, catch-up mode,
// presets, agent access and the password.
import { test, expect, sql, isCalm, makeAccount, logIn, desktopOnly } from '../support/fixtures.ts';
import { flash, open, reveal, unfold } from '../support/ui.ts';

/** Timers, typed answers and the sure check are only for cards seen before. */
function seen(cardId: number) {
  sql(
    `UPDATE cards SET state = 2, stability = 5, difficulty = 5, reps = 3,
       last_review = unixepoch() - 6 * 86400, due = unixepoch() - 60 WHERE id = ?`,
    cardId,
  );
}

test.describe('desktop', () => {
  desktopOnly();

  test('the look can be switched to simple and back', async ({ page, account }) => {
    await page.goto('/settings');
    expect(await isCalm(page)).toBe(true);
    const form = page.locator('form[action="/settings"]');
    await form.locator('input[name=desktop_look][value=simple]').check({ force: true });
    await form.getByRole('button', { name: 'Save' }).click();
    await page.goto('/');
    expect(await isCalm(page)).toBe(false);
    expect(sql<{ desktop_look: string }>('SELECT desktop_look FROM users WHERE id = ?', account.id)[0].desktop_look).toBe('simple');

    await page.goto('/settings');
    const again = page.locator('form[action="/settings"]');
    await again.locator('input[name=desktop_look][value=calm]').check({ force: true });
    await again.getByRole('button', { name: 'Save' }).click();
    await page.goto('/');
    expect(await isCalm(page)).toBe(true);
  });
});

test('phones always get the simple look', async ({ page, isMobile, viewport }) => {
  test.skip(!isMobile && (viewport?.width ?? 0) >= 960, 'phones only');
  await page.goto('/');
  expect(await isCalm(page)).toBe(false);
});

test('a larger font scale makes the text larger', async ({ page, account }) => {
  await page.goto('/');
  const before = await page.evaluate(() => parseFloat(getComputedStyle(document.documentElement).fontSize));
  sql('UPDATE users SET font_scale = 150 WHERE id = ?', account.id);
  await page.goto('/');
  const after = await page.evaluate(() => parseFloat(getComputedStyle(document.documentElement).fontSize));
  expect(after).toBeGreaterThan(before * 1.3);
});

function quietNow(userId: number) {
  const hour = Number(new Intl.DateTimeFormat('en-GB', { hour: 'numeric', hourCycle: 'h23', timeZone: 'Europe/Berlin' }).format(new Date()));
  sql('UPDATE users SET quiet_from = ?, quiet_to = ? WHERE id = ?', hour, (hour + 2) % 24, userId);
}

test('quiet hours: a rest screen, and Study anyway lifts it', async ({ page, seed, account }) => {
  const deck = await seed.deck('Night');
  await seed.card(deck, 'q', 'a');
  quietNow(account.id);
  await page.goto('/study');
  await expect(page.locator('main .study .face.front')).toHaveCount(0);
  if (await isCalm(page)) {
    // The desktop look: one dark screen over everything. The way past is
    // a faint line that asks twice.
    const wall = page.locator('.sleep-wall');
    await expect(wall).toBeVisible();
    await expect(wall.locator('h1')).toHaveText('Go to sleep.');
    expect(await page.locator('body > main').evaluate((m) => (m as HTMLElement).inert)).toBe(true);
    const faint = wall.getByText('study anyway', { exact: true });
    await expect(faint).toBeVisible();
    const color = await faint.evaluate((e) => getComputedStyle(e).color);
    expect(Number(/,\s*([\d.]+)\)$/.exec(color)?.[1] ?? 1)).toBeLessThan(0.4);
    await faint.click();
    await expect(wall).toContainText('Sleep is when');
    const yes = wall.getByRole('button', { name: /open my cards/i });
    await expect(yes).toBeHidden();
    await wall.getByText('I know. Study anyway.').click();
    await yes.click();
    await expect(page.locator('.sleep-wall')).toHaveCount(0);
    await expect(page.locator('main .study .face.front')).toHaveText('q');
    return;
  }
  await expect(page.locator('main')).toContainText('Quiet hours');
  await page.getByRole('button', { name: 'Study anyway' }).click();
  await expect(page.locator('main .study .face.front')).toHaveText('q');
});

test.describe('desktop', () => {
  desktopOnly();
  // Settings sit under the wall too; the way past is "study anyway".
  test('quiet hours also cover Settings', async ({ page, account }) => {
    quietNow(account.id);
    await page.goto('/settings');
    await expect(page.locator('.sleep-wall')).toBeVisible();
    expect(await page.locator('body > main').evaluate((m) => (m as HTMLElement).inert)).toBe(true);
  });
});

test('new cards are never timed', async ({ page, seed }) => {
  const deck = await seed.deck('Timed');
  await seed.card(deck, 'first sight', 'answer');
  sql('UPDATE presets SET time_limit = 2 WHERE id = (SELECT preset_id FROM decks WHERE id = ?)', deck);
  await page.goto('/study');
  await expect(page.locator('main .study .face.front')).toHaveText('first sight');
  await expect(page.locator('main .study .timer')).toHaveCount(0);
});

test('a timed card runs out and counts as forgotten', async ({ page, seed, account }) => {
  const deck = await seed.deck('Timed');
  seen(await seed.card(deck, 'quick!', 'answer'));
  sql('UPDATE presets SET time_limit = 2 WHERE id = (SELECT preset_id FROM decks WHERE id = ?)', deck);
  await page.goto('/study');
  const timer = page.locator('main .study .timer');
  await expect(timer).toBeVisible();
  await expect(page.locator('main .timeout-form')).toBeVisible({ timeout: 6000 });
  await expect(page.locator('main .timeout-note')).toHaveText("Time's up. This one counts as forgotten.");
  await expect(page.locator('main .study .face.back')).toBeVisible();
  await page.keyboard.press(' ');
  await expect(page.locator('main .session-end, main .study')).toBeVisible();
  await expect.poll(() => sql<{ rating: number; timed_out: number }>('SELECT rating, timed_out FROM review_log WHERE user_id = ?', account.id)).toEqual([{ rating: 1, timed_out: 1 }]);
});

test('+10 s gives a timed card more time, once', async ({ page, seed }) => {
  const deck = await seed.deck('Timed');
  seen(await seed.card(deck, 'slow', 'answer'));
  sql('UPDATE presets SET time_limit = 3 WHERE id = (SELECT preset_id FROM decks WHERE id = ?)', deck);
  await page.goto('/study');
  const extend = page.locator('main .timer .extend');
  await expect(extend).toBeVisible();
  await extend.click();
  await expect(extend).toBeHidden();
  await expect(page.locator('main .timer .timer-secs')).toHaveText(/1[0-3] s/);
  await page.waitForTimeout(3500);
  await expect(page.locator('main .timeout-form')).toBeHidden();
});

test('typed answers are shown above the answer', async ({ page, seed }) => {
  const deck = await seed.deck('Typed');
  seen(await seed.card(deck, 'Capital of Peru', 'Lima'));
  sql('UPDATE presets SET typed_answers = 1 WHERE id = (SELECT preset_id FROM decks WHERE id = ?)', deck);
  await page.goto('/study');
  const box = page.locator('main .typed-answer input');
  await expect(box).toBeFocused();
  // Keys typed into the box are text, not answers.
  await box.pressSequentially('Lima 3');
  await box.press('Enter');
  await expect(page.locator('main .typed-echo .typed-text')).toHaveText('Lima 3');
  await expect(page.locator('main .study .face.back')).toContainText('Lima');
});

test('the sure check is kept with the answer', async ({ page, seed, account }) => {
  const deck = await seed.deck('Sure');
  seen(await seed.card(deck, 'q', 'a'));
  sql('UPDATE users SET confidence_check = 1 WHERE id = ?', account.id);
  await page.goto('/study');
  await expect(page.locator('main .sure-ask')).toHaveText('Do you know it?');
  await page.keyboard.press('u');
  await expect(page.locator('main .study .face.back')).toBeVisible();
  await page.locator('main form.answers').getByRole('button', { name: /good/i }).click();
  await expect.poll(() => sql<{ confidence: number }>('SELECT confidence FROM review_log WHERE user_id = ?', account.id)).toEqual([{ confidence: 0 }]);
});

test('catch-up mode holds back new cards', async ({ page, seed, account }) => {
  const deck = await seed.deck('Backlog');
  const [old] = await seed.cards(deck, [['old', 'review'], ['new', 'card']]);
  sql('UPDATE cards SET state = 2, stability = 5, difficulty = 5, reps = 3, last_review = unixepoch() - 20 * 86400, due = unixepoch() - 86400 WHERE id = ?', old);
  await page.goto('/settings');
  const form = page.locator('form[action="/settings/study"]');
  const label = form.locator('label:has(input[name=catch_up])');
  await reveal(page, label, /Catch-up mode/);
  // Switches are labels; the box itself is drawn by CSS.
  await label.click();
  await expect(form.locator('input[name=catch_up]')).toBeChecked();
  // The desktop look saves by itself ("Saved"); the simple look has Save.
  const save = form.getByRole('button', { name: 'Save' });
  if (await save.isVisible()) await save.click();
  await expect.poll(() => sql<{ catch_up: number }>('SELECT catch_up FROM users WHERE id = ?', account.id)[0].catch_up).toBe(1);
  await page.goto('/study');
  await expect(page.locator('main .study-meta .count')).toHaveText('1 of 1');
  await expect(page.locator('main .study .face.front')).toHaveText('old');
});

test('presets: make one from a template, change it, delete it', async ({ page, account }) => {
  await page.goto('/presets');
  await expect(page.locator('main')).toContainText('Exam prep');
  await page.goto('/presets/new?template=exam');
  const form = page.locator('form[action="/presets/new"]');
  await unfold(page);
  await form.locator('input[name=name]').fill('Finals');
  await form.getByRole('button', { name: /create|save/i }).click();
  await expect(flash(page)).toHaveText('Preset created.');
  const [preset] = sql<{ id: number; desired_retention: number }>('SELECT id, desired_retention FROM presets WHERE user_id = ? AND name = ?', account.id, 'Finals');
  expect(preset.desired_retention).toBeCloseTo(0.95);

  await page.goto(`/presets/${preset.id}`);
  const edit = page.locator(`form[action="/presets/${preset.id}"]`);
  await unfold(page);
  await edit.locator('input[name=new_per_day]').fill('7');
  await edit.getByRole('button', { name: /save/i }).click();
  await expect(flash(page)).toHaveText('Preset saved.');
  expect(sql<{ new_per_day: number }>('SELECT new_per_day FROM presets WHERE id = ?', preset.id)[0].new_per_day).toBe(7);

  await page.goto(`/presets/${preset.id}`);
  await open(page.locator('details.danger'));
  await page.locator(`form[action="/presets/${preset.id}/delete"]`).getByRole('button').click();
  expect(sql('SELECT 1 FROM presets WHERE id = ?', preset.id)).toHaveLength(0);
});

test('agent access: a token is shown once and can be revoked', async ({ page, account }) => {
  await page.goto('/access');
  const form = page.locator('form[action="/access/new"]');
  await unfold(page);
  await form.locator('input[name=name]').fill('Claude');
  await form.getByRole('button').last().click();
  const token = (await page.locator('main').innerText()).match(/ainki_[0-9a-f]{20,}/)?.[0];
  expect(token, 'the new token is on the page').toBeTruthy();
  // It works against /mcp.
  const mcp = await page.request.post('/mcp', {
    headers: { Authorization: `Bearer ${token}`, Accept: 'application/json, text/event-stream', 'Content-Type': 'application/json' },
    data: { jsonrpc: '2.0', id: 1, method: 'tools/list', params: {} },
  });
  expect(mcp.status()).toBe(200);
  // Shown once only.
  await page.goto('/access');
  await expect(page.locator('main')).not.toContainText(token!);
  await page.locator('form[action$="/revoke"]').first().getByRole('button').click();
  const after = await page.request.post('/mcp', {
    headers: { Authorization: `Bearer ${token}`, Accept: 'application/json, text/event-stream', 'Content-Type': 'application/json' },
    data: { jsonrpc: '2.0', id: 1, method: 'tools/list', params: {} },
  });
  expect(after.status()).toBe(401);
  void account;
});

test('changing the password and logging out', async ({ page, account, browser }) => {
  const other = await browser.newContext();
  const elsewhere = await other.newPage();
  await logIn(elsewhere.request, account);

  await page.goto('/settings');
  const form = page.locator('form[action="/settings/password"]');
  await unfold(page);
  await form.locator('input[name=current]').fill(account.password);
  await form.locator('input[name=new]').fill('an even better password');
  await form.getByRole('button', { name: 'Change password' }).click();
  await expect(flash(page)).toHaveText('Password changed.');
  // This browser stays in, the other one is out.
  await page.goto('/stats');
  await expect(page).toHaveURL('/stats');
  await elsewhere.goto('/stats');
  await expect(elsewhere).toHaveURL(/\/login/);

  await page.goto('/settings');
  await page.locator('form[action="/logout"]').getByRole('button', { name: 'Log out' }).click();
  await expect(page).toHaveURL('/login');
  await page.locator('input[name=username]').fill(account.handle);
  await page.locator('input[name=password]').fill('an even better password');
  await page.getByRole('button', { name: 'Log in' }).click();
  await expect(page).toHaveURL('/');
  await other.close();
});

test('a wrong password at login says so', async ({ browser }) => {
  const account = makeAccount('forgetful');
  const ctx = await browser.newContext({ extraHTTPHeaders: { 'X-Forwarded-For': `10.9.${Math.floor(Math.random() * 250)}.${Math.floor(Math.random() * 250)}` } });
  const page = await ctx.newPage();
  await page.goto('/login');
  await page.locator('input[name=username]').fill(account.handle);
  await page.locator('input[name=password]').fill('not it');
  await page.getByRole('button', { name: 'Log in' }).click();
  await expect(page).toHaveURL(/\/login/);
  await expect(page.locator('main')).toContainText(/wrong|incorrect|not match/i);
  // The name typed is not kept: it has to be typed again (worth keeping?).
  await expect(page.locator('input[name=username]')).toHaveValue('');
  await ctx.close();
});
