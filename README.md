# Serana

A personal AI agent in Rust. An OpenAI-compatible model wrapped in a harness — context
management, tools, memory, subagents, skills, scheduling — reachable from a terminal REPL
or from Telegram.

## Run it

```bash
cp .env.example .env   # fill in SERANA_API_KEY and TELEGRAM_BOT_TOKEN
docker compose up -d --build
```

The REPL, against the same state:

```bash
docker compose run --rm serana
```

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
| `serana-testkit` | In-memory fakes for tests. |
| `serana-cli` | Terminal REPL. |
| `serana-tg` | Telegram bot. |

Conventions live in `.claude/skills/serana/SKILL.md`. Notes taken from the Python reference
implementation are in `docs/reference-notes.md`.
