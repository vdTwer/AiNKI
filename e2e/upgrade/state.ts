// What the v1 step leaves for the v2 step: kept in the data folder.
import fs from 'node:fs';
import path from 'node:path';
import { dataDir } from '../support/env.ts';
import type { Account } from '../support/fixtures.ts';

export interface UpgradeState {
  user: Account;
  friend: Account;
  deckId: number;
  listCardId: number;
  imageCardId: number;
  imageId: string;
  token: string;
  /** Row counts and one card's memory right after v1 wrote them. */
  counts: Record<string, number>;
  memory: { stability: number; difficulty: number; due: number; reps: number };
  storageState: string;
}

const file = path.join(dataDir, 'upgrade-state.json');
export const storageFile = path.join(dataDir, 'upgrade-browser.json');

export function save(state: UpgradeState) {
  fs.writeFileSync(file, JSON.stringify(state, null, 2));
}

export function load(): UpgradeState {
  return JSON.parse(fs.readFileSync(file, 'utf8'));
}
