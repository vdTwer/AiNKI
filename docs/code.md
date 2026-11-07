# Where things are

AiNKI is one Rust binary. `src/service.rs` holds everything a user can do; the website and the MCP server are thin layers over it.

## Server

- `src/main.rs`: configuration from the environment, the admin commands, startup.
- `src/service.rs`: decks, cards, studying, stats.
- `src/scheduler.rs`: the card lifecycle around FSRS (learning steps, day boundaries, fuzz, exam caps).
- `src/tuning.rs`: the optimizer, per-preset parameters, the monthly run, workload simulation and calibration.
- `src/presets.rs`: the built-in presets and why they look the way they do.
- `src/multi.rs`: multicards, learned item by item.
- `src/study.rs`: quiet hours, breaks, catch-up mode and the sure check.
- `src/coach.rs`: the card coach (leeches, split hints, near duplicates, splitting).
- `src/advice.rs`: the home and Stats coach, static rules over a snapshot of numbers.
- `src/insight.rs`: the week and retention per deck.
- `src/lessons.rs` and `src/lessons/*.md`: the built-in lessons, one Markdown file each.
- `src/merge.rs`: merged decks.
- `src/share.rs`: deck sharing.
- `src/invites.rs`: invitations and the admin panel.
- `src/auth.rs`: accounts, browser sessions, API tokens and access control.
- `src/oauth.rs`: the OAuth 2.1 authorization server for MCP clients.
- `src/mcp.rs`: the MCP server (rmcp, Streamable HTTP, stateless).
- `src/web.rs`: the website, rendered on the server with maud.
- `src/fuzzy.rs`: fuzzy matching for the quick find.
- `src/security.rs`: rate limiting, security headers, Origin checks, flash messages.
- `src/markdown.rs`: card Markdown, with raw HTML escaped and the output sanitized.
- `src/format.rs`: how numbers, intervals and dates are written, everywhere.
- `src/import.rs`: Anki and CSV import, CSV export.
- `src/media.rs`: image processing (resize, grayscale, Floyd–Steinberg dithering).
- `migrations/`: the SQLite schema, one file per change.

## Browser

- `src/style.css`: the simple look, which is the base everywhere.
- `src/static/theme.js`: picks the look before the page is drawn. The desktop look switches on only with JavaScript, on wide screens with a mouse that aren't e-ink, unless turned off in settings.
- `src/static/calm.css`: the desktop look, built on `style.css`.
- `src/static/desk.js`: the desktop look's extras: watercolor washes, the dot rail and quick find, stats charts.
- `src/static/app.js`: studying without page reloads and small touches. Every page works without it.
- `src/static/fonts/`: EB Garamond (SIL Open Font License), served by AiNKI itself.

## Tests

- `src/tests.rs`, `src/tests/`: Rust tests, including HTTP and MCP tests (`src/tests/http/`), Anki import, concurrency, merged decks and pinned checksums of the v1 migrations.
- `e2e/tests/`: Playwright tests against the real server binary, started on a temporary data directory by `e2e/support/serve.ts`.
- `e2e/upgrade/`: seeds data with a v1 binary, then checks it after the upgrade.
