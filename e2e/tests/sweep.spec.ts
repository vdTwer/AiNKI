// A walk over every page of a well-used account, in every look: each page
// answers, has no script errors (checked for every test), doesn't scroll
// sideways, shows no "NaN", "undefined" or raw "::", loads its images, and
// every link on it leads somewhere.
import { test, expect, sql, type Seed } from '../support/fixtures.ts';
import { png } from '../support/files.ts';

async function wellUsed(seed: Seed, userId: number) {
  const deck = await seed.deck('Languages::Spanish');
  const ids = await seed.cards(deck, [['el perro', 'the dog'], ['el gato', 'the cat'], ['la casa', 'the house']]);
  const list = await seed.card(deck, 'Three verbs', '- ser\n- estar\n- ir', { multi: 'list' });
  const other = await seed.deck('History');
  await seed.card(other, 'Hastings', '1066');
  const now = Math.floor(Date.now() / 1000);
  for (const [i, id] of ids.entries()) {
    for (let d = 40; d > 0; d -= 3 + i) {
      sql(
        `INSERT INTO review_log (user_id, card_id, deck_id, rating, reviewed_at, state_before, elapsed_days, scheduled_secs, source)
         VALUES (?, ?, ?, ?, ?, 2, 3, 259200, 'web')`,
        userId, id, deck, d % 4 ? 3 : 1, now - d * 86_400,
      );
    }
  }
  sql('UPDATE cards SET state = 2, stability = 3, difficulty = 8, lapses = 6, reps = 12, last_review = ?, due = ? WHERE id = ?', now - 86_400, now + 86_400, ids[0]);
  return { deck, card: ids[1], list, other };
}

test('every page, every link', async ({ page, seed, account }) => {
  test.setTimeout(120_000);
  const { deck, card, list } = await wellUsed(seed, account.id);
  sql('UPDATE users SET created_at = created_at - 30 * 86400 WHERE id = ?', account.id);
  // An image on a card, for the media route.
  await page.request.post(`/cards/${card}`, {
    multipart: { deck: String(deck), front: 'el gato', back: 'the cat', front_image: { name: 'cat.png', mimeType: 'image/png', buffer: png() } },
  });
  await page.context().clearCookies({ name: 'ainki_flash' });

  const start = ['/', '/study', `/decks/${deck}`, `/decks/${deck}/edit`, `/cards/${card}`, `/cards/${list}`, '/cards/new', '/decks/new',
    '/import', '/stats', `/stats?deck=${deck}`, '/settings', '/presets', '/presets/new', '/learn', '/attention', '/invites', '/access'];
  const seen = new Set<string>();
  const queue = [...start];
  const problems: string[] = [];
  while (queue.length) {
    const path = queue.shift()!;
    if (seen.has(path)) continue;
    seen.add(path);
    const res = await page.goto(path);
    const status = res?.status() ?? 0;
    if (status >= 400) {
      problems.push(`${path}: ${status}`);
      continue;
    }
    if (!(res?.headers()['content-type'] ?? '').includes('text/html')) continue;

    const report = await page.evaluate(() => {
      const doc = document.documentElement;
      const main = document.querySelector('main');
      const text = main ? main.innerText : '';
      const bad = /\bNaN\b|\bundefined\b|\[object Object\]|::/.exec(text);
      const brokenImgs = [...document.querySelectorAll('main img[src]')]
        .filter((i) => (i as HTMLImageElement).complete && (i as HTMLImageElement).naturalWidth === 0)
        .map((i) => i.getAttribute('src'));
      const links = [...document.querySelectorAll('a[href]')]
        .map((a) => a.getAttribute('href')!)
        .filter((h) => h.startsWith('/') && !h.startsWith('//'));
      return {
        wide: doc.scrollWidth - doc.clientWidth,
        bad: bad?.[0],
        brokenImgs,
        links,
        title: document.title,
        h1: document.querySelectorAll('main h1').length,
        lang: doc.lang,
      };
    });
    if (report.wide > 1) problems.push(`${path}: scrolls sideways by ${report.wide}px`);
    if (report.bad) problems.push(`${path}: shows "${report.bad}"`);
    if (report.brokenImgs.length) problems.push(`${path}: broken images ${report.brokenImgs.join(', ')}`);
    if (!/AiNKI/.test(report.title)) problems.push(`${path}: title "${report.title}"`);
    if (report.lang !== 'en') problems.push(`${path}: lang "${report.lang}"`);
    for (const link of report.links) {
      const clean = link.replace(/#.*$/, '');
      // Stay inside the account's own pages; downloads and assets are
      // fetched but not walked.
      if (!clean || seen.has(clean) || clean.startsWith('/logout')) continue;
      if (/\.(csv|css|js|png|svg|ico|woff2)(\?|$)|^\/media\//.test(clean)) {
        seen.add(clean);
        const r = await page.request.get(clean);
        if (r.status() >= 400) problems.push(`${path} -> ${clean}: ${r.status()}`);
        continue;
      }
      if (seen.size + queue.length < 150) queue.push(clean);
    }
  }
  expect(problems).toEqual([]);
  expect(seen.size).toBeGreaterThan(start.length);
});

test('static files are cached for good and versioned by content', async ({ page }) => {
  await page.goto('/');
  const assets = await page.evaluate(() =>
    [...document.querySelectorAll('link[rel=stylesheet], script[src]')].map((e) => e.getAttribute('href') ?? e.getAttribute('src')!),
  );
  expect(assets.length).toBeGreaterThanOrEqual(4);
  for (const a of assets) {
    expect(a, 'linked with a content hash').toMatch(/\?v=[0-9a-f]{6,}/);
    const res = await page.request.get(a);
    expect(res.status()).toBe(200);
    expect(res.headers()['cache-control'], a).toContain('immutable');
  }
});
