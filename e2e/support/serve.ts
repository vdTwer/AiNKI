// Starts the real AiNKI server on a fresh, empty data folder. Playwright's
// webServer runs this and stops it after the run.
//
// With E2E_KEEP_DATA=1 the folder is left as it is: the v1 -> v2
// migration job starts v2 on data that v1 wrote.
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import { binary, dataDir, port } from './env.ts';

if (process.env.E2E_KEEP_DATA !== '1') {
  fs.rmSync(dataDir, { recursive: true, force: true });
}
fs.mkdirSync(dataDir, { recursive: true });
if (!fs.existsSync(binary)) {
  console.error(`No server binary at ${binary}. Run "cargo build" first or set AINKI_BIN.`);
  process.exit(1);
}

const server = spawn(binary, [], {
  stdio: 'inherit',
  env: {
    ...process.env,
    AINKI_DATA_DIR: dataDir,
    AINKI_BIND: `127.0.0.1:${port}`,
    AINKI_TIMEZONE: 'Europe/Berlin',
    RUST_LOG: process.env.RUST_LOG ?? 'warn',
  },
});
for (const signal of ['SIGINT', 'SIGTERM'] as const) {
  process.on(signal, () => server.kill(signal));
}
server.on('exit', (code) => process.exit(code ?? 0));
