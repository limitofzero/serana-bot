//! The terminal REPL: read the environment, wire the graph, read lines.

use std::io::{BufRead, IsTerminal, Write};

use anyhow::{Context, bail};
use reedline::{DefaultPrompt, DefaultPromptSegment, Reedline, Signal};
use serana_app::{AppConfig, build_reminders, respond, text};
use serana_cli::{Input, parse};
use serana_domain::reminder::UserId;

/// Reminders are owned by a Telegram user id so that one set to here is delivered to the
/// phone by the bot. There is no sensible default: guessing would silently create reminders
/// nobody receives.
fn owner_from_env() -> anyhow::Result<UserId> {
    let raw = std::env::var("SERANA_USER_ID").unwrap_or_default();
    if raw.trim().is_empty() {
        bail!(
            "SERANA_USER_ID is not set. Set it to your numeric Telegram user id, so that \
             reminders created here are delivered to you by the bot."
        );
    }
    raw.trim()
        .parse::<i64>()
        .map(UserId::new)
        .with_context(|| format!("SERANA_USER_ID={raw:?} is not a numeric Telegram user id"))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                // A REPL's own output is the interface; logs below warn are noise in it.
                .unwrap_or_else(|_| "warn".into()),
        )
        .init();

    let config = AppConfig::from_env()?;
    let owner = owner_from_env()?;
    let wiring = build_reminders(&config).await?;

    println!("Serana — {} · {}", config.model, config.timezone);
    println!("{}\n", text::HELP);
    println!("Reminders set here are delivered by the bot, not by this session.\n");

    let mut lines = Lines::open();

    while let Some(line) = lines.next_line()? {
        match parse(&line) {
            Input::Blank => {}
            Input::Quit => break,
            Input::Unknown(word) => {
                println!("Unknown command \"{word}\". See /help for the list.\n");
            }
            Input::Command(command) => {
                println!("{}\n", respond(&wiring.reminders, owner, &command).await);
            }
        }
    }

    Ok(())
}

/// Where lines come from.
///
/// reedline needs a real terminal and errors out on a pipe, so a non-interactive run falls
/// back to plain stdin. That keeps the binary scriptable — and testable end to end, which a
/// REPL that insists on a TTY is not.
enum Lines {
    Interactive(Box<Reedline>, Box<DefaultPrompt>),
    Piped(std::io::Lines<std::io::StdinLock<'static>>),
}

impl Lines {
    fn open() -> Self {
        if std::io::stdin().is_terminal() {
            Self::Interactive(
                Box::new(Reedline::create()),
                Box::new(DefaultPrompt::new(
                    DefaultPromptSegment::Basic("serana".into()),
                    DefaultPromptSegment::Empty,
                )),
            )
        } else {
            Self::Piped(std::io::stdin().lock().lines())
        }
    }

    /// The next line, or `None` when the session is over.
    fn next_line(&mut self) -> anyhow::Result<Option<String>> {
        match self {
            Self::Interactive(editor, prompt) => {
                // `read_line` blocks; `block_in_place` keeps it off the async scheduler
                // without moving the editor into a spawned task on every keystroke.
                let signal = tokio::task::block_in_place(|| editor.read_line(&**prompt))
                    .context("could not read from the terminal")?;
                Ok(match signal {
                    Signal::Success(line) => Some(line),
                    // Ctrl-C and Ctrl-D both mean "done here", as does anything reedline
                    // adds later that is not a line of input.
                    _ => None,
                })
            }
            Self::Piped(lines) => {
                // Echo the prompt so a transcript reads like a session.
                print!("serana> ");
                std::io::stdout().flush().ok();
                lines
                    .next()
                    .transpose()
                    .context("could not read from stdin")
            }
        }
    }
}
