// Small helpers for parts of the UI that differ between the looks.
import { expect, type Page, type Locator } from '@playwright/test';

/** The flash message at the top of a page. */
export function flash(page: Page): Locator {
  return page.locator('p.notice[role=status]').first();
}

/**
 * Type a deck name. With JavaScript a name with "::" becomes chips (the
 * desktop path field); the real field still gets "A::B".
 */
export async function fillDeckName(form: Locator, name: string) {
  const typed = form.locator('input.path-input');
  if (await typed.count()) {
    await typed.fill(name);
    await typed.blur();
  } else {
    await form.locator('input[name=name], input[name=new_deck]').fill(name);
  }
}

/** Submit a form by its main button, wherever the look puts it. */
export async function submit(form: Locator, name: RegExp | string) {
  await form.getByRole('button', { name }).click();
}

/** Open a <details> fold if it is closed. */
export async function open(details: Locator) {
  if (!(await details.evaluate((d) => (d as HTMLDetailsElement).open))) {
    await details.locator('summary').click();
  }
  await expect(details).toHaveAttribute('open', '');
}

/**
 * Open every closed fold on the page: <details> (except danger zones) and
 * the desktop look's fold lines, which live outside the forms they open.
 */
export async function unfold(page: Page) {
  for (const b of await page.locator('main button.fold[aria-expanded=false]').all()) {
    if (await b.isVisible()) await b.click();
  }
  for (const d of await page.locator('main details:not([open]):not(.danger)').all()) {
    if (await d.locator('summary').isVisible()) await d.locator('summary').click();
  }
}

/**
 * Make a field visible: on the desktop, settings sit in folds that open
 * one at a time, behind a line named after the group.
 */
export async function reveal(page: Page, field: Locator, fold: RegExp) {
  if (await field.isVisible()) return;
  const line = page.locator('main button.fold').filter({ hasText: fold });
  if (await line.count()) await line.first().click();
}
