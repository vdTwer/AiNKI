// Works around a Playwright + Firefox race: now and then the first goto of
// a new page never resolves, even with waitUntil "commit", although the page
// has navigated, loaded and fired "load". (Probed on CI in November 2026:
// about 1 in 80 fresh loads.) Later navigations of the same page are fine.
//
// A patched page's first goto also asks the page itself: once its location
// is the path asked for and it has loaded, goto gets a moment more to answer
// as usual, then the page is loaded again, which Playwright follows.
// (waitForURL is no help: it loses track the same way goto does. Locators
// are blind on such a page too; only evaluate still works.)
import type { Page, Response } from '@playwright/test';
import { baseURL } from './env.ts';

const PATIENCE_MS = 4_000;
const GRACE_MS = 2_000;
const patched = new WeakSet<Page>();

export function steadyFirstLoad(page: Page): void {
  if (patched.has(page)) return;
  patched.add(page);
  const goto = page.goto.bind(page);
  let first = true;
  page.goto = async (url: string, options?: Parameters<Page['goto']>[1]): Promise<Response | null> => {
    if (!first) return goto(url, options);
    first = false;
    const target = new URL(url, baseURL).pathname;
    const navigation = goto(url, options);
    let settled = false;
    navigation.then(
      () => (settled = true),
      () => (settled = true),
    );
    // Asks the page itself, which answers even when goto has lost track.
    // It starts only after a normal load would long be done, so it never
    // touches a page mid-navigation, and stops as soon as goto answers.
    // Never rejects: if the page never gets there, goto's own timeout wins.
    const rescue = (async () => {
      await new Promise((r) => setTimeout(r, PATIENCE_MS));
      for (;;) {
        if (settled) return new Promise<never>(() => {});
        await new Promise((r) => setTimeout(r, 500));
        const there = await page
          .evaluate((path) => location.pathname === path && document.readyState === 'complete', target)
          .catch(() => false);
        if (there) break;
      }
      await new Promise((r) => setTimeout(r, GRACE_MS));
      return 'lost' as const;
    })();
    const result = await Promise.race([navigation, rescue]);
    if (result !== 'lost') return result;
    // Locators can't see the page either (only evaluate can), so go again:
    // a second load of the same page is followed as usual.
    navigation.catch(() => {});
    console.log(`First load of ${target} lost by goto though the page loaded; loading it again.`);
    return goto(url, options);
  };
}
