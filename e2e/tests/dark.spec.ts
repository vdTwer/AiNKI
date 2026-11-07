// Dark mode: every button, link and control on every page has text that
// stands out from what is behind it, at rest and under the pointer.
//
// Colors are read back through a canvas, so whatever the browser reports
// (rgb(), oklab(), color-mix()) is compared as plain sRGB. Backgrounds are
// stacked up from the element to the page, alpha included; the watercolor
// washes are left out, they are faint and move.
import type { Page } from '@playwright/test';
import { test, expect, sql, type Seed } from '../support/fixtures.ts';

test.use({ colorScheme: 'dark' });

async function seedPages(seed: Seed, userId: number) {
  const deck = await seed.deck('Languages::Spanish');
  const ids = await seed.cards(deck, [['el perro', 'the dog'], ['el gato', 'the cat']]);
  const list = await seed.card(deck, 'Three verbs', '- ser\n- estar\n- ir', { multi: 'list' });
  const now = Math.floor(Date.now() / 1000);
  for (let d = 20; d > 0; d -= 3) {
    sql(
      `INSERT INTO review_log (user_id, card_id, deck_id, rating, reviewed_at, state_before, elapsed_days, scheduled_secs, source)
       VALUES (?, ?, ?, ?, ?, 2, 3, 259200, 'web')`,
      userId, ids[0], deck, d % 4 ? 3 : 1, now - d * 86_400,
    );
  }
  return { deck, card: ids[1], list };
}

type Finding = { what: string; text: string; ratio: number; state: string };

/** Contrast of every pressable thing on the page; hovers each one too. */
async function audit(page: Page, path: string, minimum: number): Promise<string[]> {
  // Open every fold first, so the buttons inside are measured too.
  await page.evaluate(() => document.querySelectorAll('details').forEach((d) => (d.open = true)));
  const pressable = 'button, .btn, input[type=submit], summary, a, label.check, [role=button]';
  const count = await page.locator(pressable).count();
  const found: Finding[] = [];
  for (let i = 0; i < count; i++) {
    const el = page.locator(pressable).nth(i);
    if (!(await el.isVisible().catch(() => false))) continue;
    for (const state of ['rest', 'hover'] as const) {
      if (state === 'hover') {
        const ok = await el.hover({ timeout: 1500 }).then(() => true, () => false);
        if (!ok) continue;
        await page.waitForTimeout(350);
      }
      const r = await el.evaluate(measure, state);
      if (r && r.ratio < minimum) found.push(r);
    }
    await page.mouse.move(0, 0);
  }
  return found.map((f) => `${path} [${f.state}] ${f.what} "${f.text}": ${f.ratio.toFixed(2)}`);
}

// Runs in the page.
function measure(el: Element, state: string): Finding | null {
  const cv = document.createElement('canvas');
  cv.width = cv.height = 1;
  const cx = cv.getContext('2d', { willReadFrequently: true })!;
  const rgba = (c: string): [number, number, number, number] => {
    cx.clearRect(0, 0, 1, 1);
    cx.fillStyle = '#000';
    cx.fillStyle = c;
    cx.fillRect(0, 0, 1, 1);
    const d = cx.getImageData(0, 0, 1, 1).data;
    return [d[0], d[1], d[2], d[3] / 255];
  };
  const over = (top: number[], under: number[]) => [0, 1, 2].map((k) => top[k] * top[3] + under[k] * (1 - top[3]));
  // Every piece of text in the element counts; the worst one is reported.
  const texts: Element[] = [];
  const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    const p = n.parentElement;
    if (!n.textContent!.trim() || !p) continue;
    const s = getComputedStyle(p);
    if (s.visibility === 'hidden' || s.display === 'none' || p.getClientRects().length === 0) continue;
    if (!texts.includes(p)) texts.push(p);
  }
  if (!texts.length) return null;
  const lum = (c: number[]) => {
    const [r, g, b] = c.map((v) => {
      const x = v / 255;
      return x <= 0.03928 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4;
    });
    return 0.2126 * r + 0.7152 * g + 0.0722 * b;
  };
  const contrast = (textEl: Element) => {
    // Backgrounds from the page up to the text, opacity included.
    const chain: Element[] = [];
    for (let e: Element | null = textEl; e; e = e.parentElement) chain.unshift(e);
    let bg = [255, 255, 255];
    let alpha = 1;
    for (const e of chain) {
      const st = getComputedStyle(e);
      const c = rgba(st.backgroundColor);
      if (c[3] > 0) bg = over(c, bg);
      alpha *= +st.opacity;
    }
    const s = getComputedStyle(textEl);
    const fill = s.getPropertyValue('-webkit-text-fill-color');
    const raw = rgba(fill && fill !== s.color ? fill : s.color);
    raw[3] *= alpha;
    const fg = over(raw, bg);
    const [a, b] = [lum(fg), lum(bg)].sort((x, y) => y - x);
    return (a + 0.05) / (b + 0.05);
  };
  let worst = texts[0];
  let ratio = Infinity;
  for (const t of texts) {
    const r = contrast(t);
    if (r < ratio) [worst, ratio] = [t, r];
  }
  const text = (worst.textContent ?? '').trim().slice(0, 30);
  const tag = el.tagName.toLowerCase() + (el.className && typeof el.className === 'string' ? '.' + el.className.trim().split(/\s+/).join('.') : '');
  return { what: tag, text, ratio, state };
}

test('every pressable thing is readable in dark mode', async ({ page, seed, account }) => {
  test.setTimeout(300_000);
  const { deck, card, list } = await seedPages(seed, account.id);
  const paths = ['/', `/decks/${deck}`, `/decks/${deck}/edit`, `/cards/${card}`, `/cards/${list}`, '/cards/new', '/decks/new',
    '/import', '/stats', '/settings', '/presets', '/presets/new', '/learn', '/attention', '/invites', '/access', '/study'];
  const problems: string[] = [];
  // 3:1, the bar for large text and controls: the quiet grey words pass it.
  const min = 3;
  for (const path of paths) {
    await page.goto(path);
    problems.push(...(await audit(page, path, min)));
  }
  // Studying, card by card to the end: front, answer side, multicard items.
  await page.goto('/study');
  for (let n = 0; n < 6; n++) {
    const show = page.locator('main .study .reveal a.show');
    if (!(await show.isVisible().catch(() => false))) break;
    await show.click();
    await page.waitForTimeout(400);
    problems.push(...(await audit(page, `/study (answer ${n})`, min)));
    const done = page.locator('main form.list-answer').getByRole('button', { name: /done/i });
    if (await done.isVisible().catch(() => false)) await done.click();
    else await page.locator('main .answers .btn').last().click();
    await page.waitForTimeout(600);
    problems.push(...(await audit(page, `/study (after ${n})`, min)));
  }
  expect(problems).toEqual([]);
});
