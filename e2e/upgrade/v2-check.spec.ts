// Runs against v2 on the folder v1 wrote: nothing lost, everything works.
import { test, expect } from '@playwright/test';
import { logIn, sql } from '../support/fixtures.ts';
import { load } from './state.ts';

const state = load();

test('rows and memory are exactly as v1 left them', () => {
  for (const [table, n] of Object.entries(state.counts)) {
    expect(sql<{ n: number }>(`SELECT COUNT(*) AS n FROM ${table}`)[0].n, table).toBe(n);
  }
  const [memory] = sql("SELECT stability, difficulty, due, reps FROM cards WHERE user_id = ? AND front = 'el gato'", state.user.id);
  expect(memory).toEqual(state.memory);
  const applied = sql<{ version: number; success: number }>('SELECT version, success FROM _sqlx_migrations ORDER BY version');
  expect(applied.every((m) => m.success === 1)).toBe(true);
  expect(applied.at(-1)!.version).toBeGreaterThan(8);
});

test.describe('logged in from v1', () => {
  test.use({ storageState: state.storageState });

  test('the v1 session is still logged in and studying goes on', async ({ page }) => {
    await page.goto('/');
    await expect(page).toHaveURL('/');
    await expect(page.locator('main')).toContainText('Spanish');
    await expect(page.locator('main')).not.toContainText('::');
    expect(await page.evaluate(() => document.documentElement.classList.contains('calm'))).toBe(true);

    const before = sql<{ n: number }>('SELECT COUNT(*) AS n FROM review_log WHERE user_id = ?', state.user.id)[0].n;
    await page.goto('/study');
    const show = page.locator('main .reveal a.show');
    if (await show.isVisible()) {
      await show.click();
      // v1's "four buttons" setting carries over.
      const form = page.locator('main form.answers, main form.list-answer').first();
      await form.getByRole('button').last().click();
      await expect.poll(() => sql<{ n: number }>('SELECT COUNT(*) AS n FROM review_log WHERE user_id = ?', state.user.id)[0].n).toBe(before + 1);
    }
  });

  test('every v2 page works on v1 data', async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', (e) => errors.push(e.message));
    for (const path of ['/', `/decks/${state.deckId}`, `/cards/${state.listCardId}`, `/cards/${state.imageCardId}`, '/stats', '/settings', '/presets', '/learn', '/attention', '/invites', '/access', '/import', '/cards/new']) {
      const res = await page.goto(path);
      expect(res?.status(), path).toBe(200);
      await expect(page.locator('main'), path).not.toContainText(/NaN|undefined/);
    }
    expect(errors).toEqual([]);
    const img = await page.request.get(`/media/${state.imageId}`);
    expect(img.status()).toBe(200);
  });

  test('the list card kept its items and their memory', async ({ page }) => {
    const items = sql<{ text: string; reps: number }>('SELECT text, reps FROM card_items WHERE card_id = ? ORDER BY position', state.listCardId);
    expect(items.map((i) => i.text)).toEqual(['ser', 'estar', 'ir']);
    await page.goto(`/cards/${state.listCardId}`);
    await expect(page.locator('input[name=multi][value=list]')).toBeChecked();
  });

  test('a v1 account old enough can invite at once', async ({ page }) => {
    await page.goto('/invites');
    await page.getByRole('button', { name: 'Make an invite link' }).click();
    await expect(page.locator('pre.copy')).toContainText('/invite/');
  });
});

test('the v1 password still logs in', async ({ page }) => {
  await page.goto('/login');
  await page.locator('input[name=username]').fill(state.user.handle);
  await page.locator('input[name=password]').fill(state.user.password);
  await page.getByRole('button', { name: 'Log in' }).click();
  await expect(page).toHaveURL('/');
});

test('the v1 agent token works with the v2 tools', async ({ request }) => {
  const call = (method: string, params: object) =>
    request.post('/mcp', {
      headers: { Authorization: `Bearer ${state.token}`, Accept: 'application/json, text/event-stream', 'Content-Type': 'application/json' },
      data: { jsonrpc: '2.0', id: 1, method, params },
    });
  const list = await call('tools/list', {});
  expect(list.status()).toBe(200);
  const names = ((await list.json()).result.tools as { name: string }[]).map((t) => t.name);
  expect(names).toContain('get_coach_advice');
  const overview = await call('tools/call', { name: 'get_overview', arguments: {} });
  const text = (await overview.json()).result.content[0].text;
  expect(JSON.parse(text).user).toBe(state.user.handle);
});

test('the share offered in v1 can be accepted in v2', async ({ page }) => {
  await logIn(page.request, state.friend);
  await page.goto('/');
  await page.getByRole('button', { name: 'Accept' }).click();
  expect(sql('SELECT name FROM decks WHERE user_id = ?', state.friend.id)).toEqual([{ name: 'Languages::Spanish' }]);
});
