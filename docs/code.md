# Where things are

AiNKI is one Rust binary. `src/service/` holds everything a user can do; the website and the MCP server are thin layers over it.

## Server

- `src/main.rs`: configuration from the environment, the admin commands, startup.
- `src/service/`: everything a user can do, one file per area (presets, decks, cards, studying, stats, settings), each with its types.
- `src/scheduler.rs`: the card lifecycle around FSRS (learning steps, day boundaries, fuzz, exam caps).
- `src/tuning.rs`: the optimizer, per-preset parameters, the monthly run, workload simulation and calibration.
- `src/presets.rs`: the built-in presets and why they look the way they do.
- `src/multi.rs`: multicards, learned item by item.
- `src/study.rs`: quiet hours, breaks, catch-up mode and the sure check.
- `src/breaks.rs`: when a study break is due, learned from each user's own rhythm.
- `src/outlook.rs`: the coming week's forecast: how many cards each day will ask for.
- `src/coach.rs`: the card coach (leeches, split hints, near duplicates, splitting).
- `src/advice.rs`: the home and Stats coach, static rules over a snapshot of numbers.
- `src/insight.rs`: the week and retention per deck.
- `src/merge.rs`: merged decks.
- `src/share.rs`: deck sharing.
- `src/invites.rs`: invitations and the admin panel.
- `src/auth.rs`: accounts, browser sessions, API tokens and access control.
- `src/oauth.rs`: the OAuth 2.1 authorization server for MCP clients.
- `src/mcp.rs`: the MCP server (rmcp, Streamable HTTP, stateless).
- `src/web/mod.rs`: the website's routes and shared plumbing (layout, forms, flash messages), rendered on the server with maud.
- `src/web/pages/*.rs`: one file per area of the site: home, study, decks, cards, coach, stats, settings and the rest.
- `src/fuzzy.rs`: fuzzy matching for the quick find.
- `src/security.rs`: rate limiting, security headers, Origin checks, flash messages.
- `src/markdown.rs`: card Markdown, with raw HTML escaped and the output sanitized.
- `src/format.rs`: how numbers, intervals and dates are written, everywhere.
- `src/num.rs`: every numeric conversion, named and checked, so no `as` cast hides a wrap.
- `src/import.rs`: Anki and CSV import, CSV export.
- `src/media.rs`: image processing (resize, grayscale, Floyd–Steinberg dithering).
- `migrations/`: the SQLite schema, one file per change.

## Browser

- `src/style.css`: the one look, for every screen. Plain modern CSS: `@layer`, nesting, `light-dark()`, `:has()` and container queries; e-ink is one `@media (monochrome)` override.
- `src/web/charts.rs`: the stats charts, drawn on the server as SVG with a `<title>` on every mark.
- `src/static/app.js`: studying without page reloads and small touches. Every page works without it.
- `src/static/parts.js`: custom elements around server markup: `<ainki-find>` (the quick find dialog over the `/find` page) and `<ainki-break>` (a break that counts down). Plain JavaScript with JSDoc types, checked by `tsc --checkJs` in CI (`e2e/tsconfig.parts.json`); there is no build step.

## Tests

- `src/tests.rs`, `src/tests/`: Rust tests, including HTTP and MCP tests (`src/tests/http/`), Anki import, concurrency, merged decks and pinned checksums of the v1 migrations.
- `e2e/tests/`: Playwright tests against the real server binary, started on a temporary data directory by `e2e/support/serve.ts`.
- `e2e/upgrade/`: seeds data with a v1 binary, then checks it after the upgrade.
