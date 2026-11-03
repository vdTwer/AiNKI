// Test fixtures: a fresh account per test, logged in, in the look the
// project asks for, plus helpers that set up data the quick way.
import { execFileSync } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { DatabaseSync } from 'node:sqlite';
import path from 'node:path';
import { test as base, expect, type Page, type APIRequestContext } from '@playwright/test';
import { binary, dataDir } from './env.ts';

export { expect };

export type Look = 'calm' | 'simple';

export interface Account {
  id: number;
  name: string;
  handle: string;
  password: string;
}

/** Run the server's command line on the test data folder. */
export function cli(...args: string[]): string {
  return execFileSync(binary, args, {
    env: { ...process.env, AINKI_DATA_DIR: dataDir, AINKI_TIMEZONE: 'Europe/Berlin', RUST_LOG: 'error' },
    encoding: 'utf8',
  });
}

/** Run SQL on the server's database: for time travel and quick setup. */
export function sql<T = Record<string, unknown>>(query: string, ...params: (string | number | null)[]): T[] {
  const db = new DatabaseSync(path.join(dataDir, 'ainki.db'));
  try {
    db.exec('PRAGMA busy_timeout = 10000');
    const stmt = db.prepare(query);
    if (/^\s*(select|with)\b/i.test(query) || /\breturning\b/i.test(query)) {
      return stmt.all(...params) as T[];
    }
    stmt.run(...params);
    return [];
  } finally {
    db.close();
  }
}

export function makeAccount(prefix = 'user', options: { admin?: boolean; look?: Look; ageDays?: number } = {}): Account {
  const name = `${prefix}${randomBytes(4).toString('hex')}`;
  const password = 'correct horse battery';
  const out = cli(options.admin ? 'create-admin' : 'create-user', name, password);
  const handle = /created (?:admin )?(\S+)/.exec(out)?.[1]?.replace(/;$/, '');
  if (!handle) throw new Error(`create-user said: ${out}`);
  const [{ id }] = sql<{ id: number }>('SELECT id FROM users WHERE username = ?', name);
  if (options.look) sql('UPDATE users SET desktop_look = ? WHERE id = ?', options.look, id);
  if (options.ageDays) sql('UPDATE users SET created_at = created_at - ? WHERE id = ?', options.ageDays * 86_400, id);
  return { id, name, handle, password };
}

/** Log a browser context in without the login page (that has its own tests). */
export async function logIn(request: APIRequestContext, account: Account): Promise<void> {
  const res = await request.post('/login', {
    form: { username: account.handle, password: account.password },
    maxRedirects: 0,
  });
  expect(res.status(), 'login answers with a redirect').toBe(303);
}

/** Quick data: decks and cards through the website's own forms. */
export class Seed {
  constructor(private readonly request: APIRequestContext, private readonly account: Account) {}

  async deck(name: string, extra: Record<string, string> = {}): Promise<number> {
    const res = await this.request.post('/decks/new', { form: { name, description: '', ...extra }, maxRedirects: 0 });
    expect(res.status(), `deck ${name} created`).toBe(303);
    const [{ id }] = sql<{ id: number }>(
      'SELECT id FROM decks WHERE user_id = ? AND name = ? ORDER BY id DESC LIMIT 1',
      this.account.id,
      name,
    );
    return id;
  }

  async card(deckId: number, front: string, back: string, extra: Record<string, string> = {}): Promise<number> {
    const res = await this.request.post('/cards/new', {
      multipart: { deck: String(deckId), front, back, ...extra },
      maxRedirects: 0,
    });
    expect(res.status(), `card ${front} created`).toBe(303);
    const [{ id }] = sql<{ id: number }>(
      'SELECT id FROM cards WHERE deck_id = ? AND front = ? ORDER BY id DESC LIMIT 1',
      deckId,
      front,
    );
    return id;
  }

  async cards(deckId: number, pairs: [string, string][]): Promise<number[]> {
    const ids = [];
    for (const [f, b] of pairs) ids.push(await this.card(deckId, f, b));
    return ids;
  }
}

/** Whether the page is drawn in the desktop (Calm) look. */
export async function isCalm(page: Page): Promise<boolean> {
  return page.evaluate(() => document.documentElement.classList.contains('calm'));
}

/** Errors the page threw or logged: every test fails on any of them. */
function watchErrors(page: Page, errors: string[]) {
  page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`));
  page.on('console', (m) => {
    if (m.type() === 'error') errors.push(`console: ${m.text()}`);
  });
}

type Fixtures = {
  look: Look;
  account: Account;
  seed: Seed;
  /** Push a pattern here when a test provokes an error on purpose. */
  allowErrors: RegExp[];
  pageErrors: string[];
};

export const test = base.extend<Fixtures>({
  look: ['calm', { option: true }],
  // Every test starts logged in; logged-out pages use a new context.
  account: [
    async ({ look, page }, use) => {
      const account = makeAccount('user', { look });
      await logIn(page.request, account);
      await use(account);
    },
    { auto: true },
  ],
  seed: async ({ page, account }, use) => {
    await use(new Seed(page.request, account));
  },
  allowErrors: async ({}, use) => {
    await use([]);
  },
  pageErrors: [
    async ({ page, allowErrors }, use) => {
      const errors: string[] = [];
      watchErrors(page, errors);
      await use(errors);
      const unexpected = errors.filter((e) => !allowErrors.some((re) => re.test(e)));
      expect(unexpected, 'no script errors on the page').toEqual([]);
    },
    { auto: true },
  ],
});

/** A form the server refused shows up in the console as a 4xx. */
export const REFUSED = /Failed to load resource: the server responded with a status of 4\d\d/;

/** Skip unless the browser draws the desktop look (desktop projects with look=calm). */
export function desktopOnly() {
  test.skip(({ look, isMobile, viewport }) => look !== 'calm' || isMobile || (viewport?.width ?? 0) < 960, 'the desktop look only');
}
