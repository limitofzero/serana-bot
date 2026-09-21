# Serana

A personal AI agent in Rust. An OpenAI-compatible model wrapped in a harness — context
management, tools, memory, subagents, skills, scheduling — reachable from a terminal REPL
or from Telegram.

## What it does today

Reminders, set in plain language over Telegram:

```
/reminder каждый месяц 20 число - писать мне что надо оформить invoice
/reminders
/reminder_delete a3f9k2xy
```

A model reads the request into a schedule; the schedule is stored and read back to you in
words, not as a cron expression. A scheduler ticks beside the bot and delivers each reminder
when it comes round. Downtime collapses missed firings into one message rather than replaying
them.

The bot answers only the Telegram user ids in `SERANA_ALLOWED_USER_IDS`, and an empty list
means nobody.

## Run it

```bash
cp .env.example .env   # fill in SERANA_API_KEY and TELEGRAM_BOT_TOKEN
docker compose up -d --build
```

The REPL, against the same state:

```bash
docker compose run --rm serana
```

It manages reminders and does not deliver them — a second scheduler over the same
database would send everything twice. Set `SERANA_USER_ID` to your Telegram id and the
bot delivers what you create there.

State (conversations, memory, any future index) lives in the `serana-data` volume, mounted
at `/data`.

## Develop

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run --bin serana
```

## Layout

The workspace is layered by crate so cargo enforces the dependency direction: `services`
and `adapters` both point at `domain` and never at each other; only the binaries know both.

| Crate | Role |
| --- | --- |
| `serana-domain` | Types and port traits. Pure — no I/O. |
| `serana-services` | The turn loop and orchestration. |
| `serana-adapters` | Provider client, repositories — everything that talks to the world. |
| `serana-app` | Shared by both frontends: commands, replies, config, wiring. |
| `serana-testkit` | In-memory fakes for tests. |
| `serana-cli` | Terminal REPL. Manages reminders; does not deliver them. |
| `serana-tg` | Telegram bot. |

## Reading

| File | What it is |
| --- | --- |
| `docs/concept.md` | What serana is trying to be: the `LLM + harness` model this project follows, the six parts of that harness, and how far along each one is. |
| `docs/reminders.md` | How a reminder actually flows through the crates — the one feature that is finished. |
| `docs/reference-notes.md` | Findings from reading the Python implementation this reimplements. |
| `.claude/skills/serana/SKILL.md` | The engineering rules: layering, the approved crate stack, the testing contract. Binding for every change. |
