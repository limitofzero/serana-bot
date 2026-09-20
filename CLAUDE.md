# serana

A personal AI agent in Rust: an OpenAI-compatible LLM wrapped in a harness (context
management, tools, memory, subagents, skills, scheduling), delivered over a CLI REPL and a
Telegram bot.

## Working in this repo

Load the `serana` skill (`.claude/skills/serana/SKILL.md`) before any code change. It is
binding and covers:

- **Library first** — use a maintained crate instead of hand-rolling; approved stack listed.
- **Ports and adapters** — `domain` (pure types + port traits), `services` (logic, depends on
  domain only), `adapters` (I/O, depends on domain only), binaries wire them together.
- **Testing** — unit tests in every module against fakes, adapters against `wiremock`, e2e
  over a scripted provider. No test touches the network or needs an API key.

Before claiming work is done: `cargo fmt --all`, `cargo clippy --workspace --all-targets --
-D warnings`, `cargo test --workspace` — all clean.
