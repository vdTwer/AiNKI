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
