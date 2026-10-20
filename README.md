# AiNKI

An AI-native flashcard service. Like Anki, but:

- **AI-native.** Agents get a full MCP API. Anything you can do on the website, an agent can do too, within the permissions you give its token.
- **FSRS by default.** Scheduling uses FSRS-6 with researched defaults and presets for common situations. One button fits the parameters to your own memory.
- **Simple cards.** A front and a back. Each is Markdown text plus an optional image. No note types, no templates.
- **Web first, mobile first.** One website that works well on a phone and on e-ink readers: grayscale, high contrast, no animations, large tap targets, no JavaScript required. Images are dithered to 16 gray levels on upload.

Accounts are a username and password. Usernames get a Discord-style tag (`kolja#4821`), so two people can pick the same name.

## Run it

```sh
git clone https://github.com/vdTwer/AiNKI.git
cd AiNKI
docker compose up -d
```

Open <http://localhost:8080> and sign up. Data lives in the `ainki-data` Docker volume.

Settings go in a `.env` file next to `docker-compose.yml` (all optional):

| Variable | Default | Meaning |
| --- | --- | --- |
| `AINKI_PORT` | `8080` | Port on the host. |
| `AINKI_ALLOW_SIGNUP` | `true` | Set to `false` once everyone has an account. |
| `AINKI_TIMEZONE` | `UTC` | Default timezone for new accounts. Each user can change theirs. |
| `AINKI_PUBLIC_URL` | (from request) | E.g. `https://cards.example.com`. Used in the MCP setup snippet. |
| `AINKI_SECURE_COOKIES` | `false` | Set to `true` when served over HTTPS. |

To serve it on the internet, put it behind a reverse proxy that terminates TLS (Caddy, nginx, Traefik), and set `AINKI_PUBLIC_URL` and `AINKI_SECURE_COOKIES=true`.

### Admin commands

There's no email, so password resets are done by the admin:

```sh
docker compose exec ainki ainki list-users
docker compose exec ainki ainki reset-password 'kolja#4821' 'new-password'
```

### Backups

Everything is in the volume: `ainki.db` (SQLite) and `media/`. To make a consistent copy:

```sh
docker compose stop && docker run --rm -v ainki_ainki-data:/data -v "$PWD":/backup debian:trixie-slim tar czf /backup/ainki-backup.tgz -C /data . && docker compose start
```

## Connect an agent (MCP)

1. On the website go to **Settings → Agent access** and create a token. Choose:
   - **Read & write**: allows adding, editing and deleting. Without it the token is read-only.
   - **Can review**: lets the agent quiz you and record your answers. Those reviews are labelled as agent reviews.
   - **All decks** or a list of decks. A token limited to some decks can't create or delete decks or change presets.
2. Point your agent at `http(s)://<your host>/mcp`, sending the token as `Authorization: Bearer <token>`. The page shows ready-made snippets, for example:

```sh
claude mcp add --transport http ainki https://cards.example.com/mcp --header "Authorization: Bearer ainki_…"
```

Agents can never change your password or settings, or create tokens.

Tools: `get_overview`, `list_decks`, `create_deck`, `update_deck`, `delete_deck`, `list_cards`, `get_card`, `create_cards`, `update_card`, `delete_cards`, `set_cards_suspended`, `reset_cards`, `upload_image`, `get_image`, `get_next_card`, `answer_card`, `undo_last_answer`, `get_stats`, `list_presets`, `create_preset`, `update_preset`, `delete_preset`, `optimize_fsrs`, `import_csv`, `import_apkg`, `export_deck_csv`.

## Scheduling

AiNKI uses the [FSRS](https://github.com/open-spaced-repetition/fsrs4anki/wiki/The-Algorithm) model through the [`fsrs`](https://crates.io/crates/fsrs) crate (FSRS-6). FSRS tracks each card's *stability* (how long until recall drops to 90%) and *difficulty*. It schedules the next review for when your chance of recalling the card drops to your *desired retention*.

By default there are two answer buttons, **Again** (forgot) and **Good** (remembered). FSRS works well with two buttons. You can turn on **Hard** and **Easy** in settings.

### Presets

New accounts get these presets:

| Preset | Retention | New/day | Reviews/day | Learning steps | Use for |
| --- | --- | --- | --- | --- | --- |
| Standard | 90% | 20 | 200 | 10m | Most material. |
| Exam prep | 95% | 50 | 9999 | 1m 10m | An upcoming test. Set an exam date on the deck so no card is scheduled after it. |
| Language vocabulary | 90% | 30 | 300 | 10m | Many small cards every day. |
| Relaxed | 85% | 10 | 100 | 10m | Long-term learning on a small time budget. |
| Maintenance | 80% | 0 | 100 | 10m | Keeping old knowledge with minimal effort. |

How these were chosen, following the FSRS project's guidance:

- **Default parameters.** The FSRS-6 defaults are fitted on about 10,000 Anki collections. After about 1,000 reviews, use **Settings → Optimize** to fit FSRS to your own history. Re-run it every month or so.
- **Desired retention.** 0.80 to 0.95 is the useful range. Below about 0.80 you relearn so much that total work goes up. Above 0.95, work climbs steeply: 0.95 costs about twice the reviews of 0.90. Around 0.85 is close to the least work per card remembered.
- **Learning steps.** Keep them short (minutes, under a day). Spacing across days is FSRS's job; long learning steps work against the model.
- **Daily limits.** Each new card leads to about 10 reviews in its first month, so the review limit should be at least 10 times the new card limit.
- **Fuzz.** Intervals get a small random spread so cards added together don't stay due together.

### Import from Anki

Upload an `.apkg` or `.colpkg` under **Import**. Each card keeps the first two fields of its note (HTML stripped, Markdown-ish formatting) and the first image on each side. Reversed cards swap the fields, and cloze notes become one card per gap. Review history is imported, so FSRS memory states are computed from your real history and the optimizer can use it. Suspended cards stay suspended.

CSV import takes one card per line (`front,back`). Commas, semicolons or tabs all work.

## Development

```sh
cargo test
AINKI_DATA_DIR=./data cargo run
```

The code:

- `src/service.rs`: everything a user can do (decks, cards, studying, stats, optimizer). The website and MCP server are thin layers over it.
- `src/scheduler.rs`: card lifecycle around FSRS (learning steps, day boundaries, fuzz, exam caps).
- `src/web.rs`, `src/style.css`: the website (server-rendered with maud).
- `src/mcp.rs`: the MCP server (rmcp, Streamable HTTP, stateless).
- `src/import.rs`: Anki and CSV import, CSV export.
- `src/media.rs`: image processing (resize, grayscale, Floyd–Steinberg dithering).

## Roadmap

- Sharing decks between users, and public decks.
- Offline studying (service worker plus a sync queue).
- OAuth for MCP clients, in addition to tokens.
- "Compute optimal retention" (CMRR) suggestions per preset.
- Optional automatic monthly optimization.
- Export to `.apkg`.

## License

MIT. FSRS scheduling comes from [fsrs-rs](https://github.com/open-spaced-repetition/fsrs-rs) (BSD-3-Clause). AiNKI contains no code from Anki itself (AGPL); it is compatible with Anki's file format only.
