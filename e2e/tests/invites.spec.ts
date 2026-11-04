// Invitations: a week after joining, one link a week; the page behind a
// link makes an account once; the admin sees only the admin panel.
import { test, expect, sql, makeAccount, logIn } from '../support/fixtures.ts';
import type { Browser, Request } from '@playwright/test';
import { flash } from '../support/ui.ts';

let ipCounter = 1;
// Requests of visitor pages that have not finished yet. In desktop Firefox
// a visit sometimes never reaches its load event; if a test fails, these
// name what it was still waiting for.
const pending = new Map<Request, string>();
/** A visitor from an address of their own (signups are limited per address). */
async function visitor(browser: Browser) {
  const n = ipCounter++ + Math.floor(Math.random() * 200) * 250;
  const context = await browser.newContext({
    extraHTTPHeaders: { 'X-Forwarded-For': `10.${(n >> 16) & 255}.${(n >> 8) & 255}.${n & 255}` },
  });
  context.on('request', (r) => pending.set(r, `${r.method()} ${r.url()} (${r.resourceType()})`));
  context.on('requestfinished', (r) => pending.delete(r));
  context.on('requestfailed', (r) => pending.set(r, `${pending.get(r) ?? r.url()} failed: ${r.failure()?.errorText}`));
  return { context, page: await context.newPage() };
}

test.beforeEach(() => pending.clear());
test.afterEach(({}, info) => {
  if (info.status !== info.expectedStatus && pending.size) {
    console.log(`Unfinished requests in "${info.title}":\n  ${[...pending.values()].join('\n  ')}`);
  }
});

test('a new account waits a week before inviting', async ({ page }) => {
  await page.goto('/invites');
  await expect(page.locator('main')).toContainText(/You can invite someone from .*, a week after you joined\./);
  await expect(page.getByRole('button', { name: /make an invite link/i })).toHaveCount(0);
});

test('an invite link makes one account, then is used up', async ({ page, account, browser }) => {
  sql('UPDATE users SET created_at = created_at - 8 * 86400 WHERE id = ?', account.id);
  await page.goto('/settings');
  await page.getByRole('link', { name: /invite someone/i }).click();
  await expect(page).toHaveURL('/invites');
  await page.getByRole('button', { name: 'Make an invite link' }).click();
  const link = (await page.locator('pre.copy').first().innerText()).trim();
  expect(link).toMatch(/\/invite\/[\w-]+$/);
  // One a week: the next one is a week away.
  await expect(page.locator('main')).toContainText(/Your next invite is ready/);
  await expect(page.getByRole('button', { name: /make another link/i })).toHaveCount(0);

  const path = new URL(link).pathname;
  const guest = await visitor(browser);
  await guest.page.goto(path);
  await expect(guest.page.locator('h1')).toHaveText(`${account.name.charAt(0).toUpperCase()}${account.name.slice(1)} invited you.`);
  const name = `guest${Date.now() % 100000}`;
  await guest.page.locator('input[name=username]').fill(name);
  await guest.page.locator('input[name=password]').fill('a long enough password');
  await guest.page.getByRole('button').last().click();
  await expect(guest.page).toHaveURL('/');
  await expect(flash(guest.page)).toContainText(`Your name here is ${name}#`);
  const [joined] = sql<{ invited_by: number }>('SELECT invited_by FROM users WHERE username = ?', name);
  expect(joined.invited_by).toBe(account.id);

  // The link is used up: a second visitor gets a friendly dead end.
  const late = await visitor(browser);
  const res = await late.page.goto(path);
  expect(res?.status()).toBeLessThan(500);
  await expect(late.page.locator('input[name=username]')).toHaveCount(0);
  await expect(late.page.locator('main')).toContainText(/no longer works|used|expired/i);

  // The inviter sees who joined.
  await page.goto('/invites');
  await expect(page.locator('main')).toContainText('Joined through you');
  await expect(page.locator('main')).toContainText(name);
  await guest.context.close();
  await late.context.close();
});

test('a link can be taken back', async ({ page, account, browser }) => {
  sql('UPDATE users SET created_at = created_at - 8 * 86400 WHERE id = ?', account.id);
  await page.goto('/invites');
  await page.getByRole('button', { name: 'Make an invite link' }).click();
  const path = new URL((await page.locator('pre.copy').first().innerText()).trim()).pathname;
  await page.getByRole('button', { name: 'Take it back' }).click();
  await expect(flash(page)).toHaveText('The link no longer works.');
  const guest = await visitor(browser);
  await guest.page.goto(path);
  await expect(guest.page.locator('input[name=username]')).toHaveCount(0);
  await guest.context.close();
});

test('a refused name on the invite page keeps the link usable', async ({ page, account, browser }) => {
  sql('UPDATE users SET created_at = created_at - 8 * 86400 WHERE id = ?', account.id);
  await page.goto('/invites');
  await page.getByRole('button', { name: 'Make an invite link' }).click();
  const path = new URL((await page.locator('pre.copy').first().innerText()).trim()).pathname;
  const guest = await visitor(browser);
  await guest.page.goto(path);
  await guest.page.locator('input[name=username]').fill('x'); // too short for the server
  await guest.page.locator('input[name=username]').evaluate((i) => i.removeAttribute('minlength'));
  await guest.page.locator('input[name=password]').fill('a long enough password');
  await guest.page.getByRole('button').last().click();
  await expect(guest.page.locator('input[name=username]')).toBeVisible();
  // The link still works for a good name.
  await guest.page.locator('input[name=username]').fill(`ok${Date.now() % 100000}`);
  await guest.page.locator('input[name=password]').fill('a long enough password');
  await guest.page.getByRole('button').last().click();
  await expect(guest.page).toHaveURL('/');
  await guest.context.close();
});

test('logged in, an invite link says it is for someone new', async ({ page, account, browser }) => {
  const inviter = makeAccount('inviter', { ageDays: 8 });
  const ctx = await browser.newContext();
  const p = await ctx.newPage();
  await logIn(p.request, inviter);
  await p.goto('/invites');
  await p.getByRole('button', { name: 'Make an invite link' }).click();
  const path = new URL((await p.locator('pre.copy').first().innerText()).trim()).pathname;
  await page.goto(path);
  await expect(page.locator('body')).toContainText(`You're logged in as ${account.handle}. This link is for someone new.`);
  await ctx.close();
});

test('the admin sees only the admin panel', async ({ browser }) => {
  const admin = makeAccount('admin', { admin: true });
  const someone = makeAccount('someone', { ageDays: 30 });
  const ctx = await browser.newContext();
  const page = await ctx.newPage();
  await logIn(page.request, admin);
  for (const path of ['/', '/study', '/stats', '/settings', '/cards/new']) {
    await page.goto(path);
    await expect(page, path).toHaveURL('/admin');
  }
  await expect(page.locator('h1')).toHaveText('Accounts');
  await expect(page.locator('main')).toContainText('Who invited whom');
  await expect(page.locator('main table')).toContainText(someone.handle);

  // The admin invites without a weekly limit.
  await page.goto('/invites');
  await page.getByRole('button', { name: /make an invite link/i }).click();
  await expect(page.getByRole('button', { name: /make another link/i })).toBeVisible();
  const guest = await visitor(browser);
  await guest.page.goto(new URL((await page.locator('pre.copy').first().innerText()).trim()).pathname);
  await expect(guest.page.locator('h1')).toHaveText("You're invited.");
  await guest.context.close();
  await ctx.close();
});

test('signing up without an invite is closed once accounts exist', async ({ browser }) => {
  const guest = await visitor(browser);
  await guest.page.goto('/signup');
  await expect(guest.page.locator('form[action="/signup"]')).toHaveCount(0);
  await guest.context.close();
});
