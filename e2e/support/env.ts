// Where the server under test runs and keeps its data. Shared by the
// config, the server launcher and the fixtures, so every worker agrees.
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));

export const repoRoot = path.resolve(here, '..', '..');
export const port = Number(process.env.E2E_PORT ?? 18_765);
export const baseURL = `http://127.0.0.1:${port}`;
export const dataDir = process.env.E2E_DATA_DIR ?? path.join(os.tmpdir(), `ainki-e2e-${port}`);
/** The server binary: a debug build by default, AINKI_BIN to override. */
export const binary = process.env.AINKI_BIN ?? path.join(repoRoot, 'target', 'debug', 'ainki');
