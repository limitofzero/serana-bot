//! The Telegram bot: read the environment, wire the graph, dispatch.

use std::sync::Arc;

use serana_app::{AppConfig, Command, Reminders, build_reminders, build_scheduler, respond, text};
use serana_domain::conversation::ConversationId;
use serana_domain::reminder::UserId;
use serana_tg::TelegramCommand;
use serana_tg::config::TelegramConfig;
use serana_tg::notifier::TelegramNotifier;
use teloxide::prelude::*;
use teloxide::utils::command::BotCommands;

struct AppState {
    reminders: Reminders,
    telegram: TelegramConfig,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // A missing .env is normal: in Docker the variables come from the environment.
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let config = AppConfig::from_env()?;
    let telegram = TelegramConfig::from_env()?;
    if telegram.allowed_users.is_empty() {
        tracing::warn!(
            "SERANA_ALLOWED_USER_IDS is empty, so the bot will answer nobody. \
             Set it to your Telegram user id."
        );
    }

    let wiring = build_reminders(&config).await?;
    let bot = Bot::new(&telegram.token);

    // One scheduler ticking beside the dispatcher. Both share the database; SQLite in WAL
    // mode lets the tick read while a command writes.
    let scheduler = build_scheduler(wiring.repository, TelegramNotifier::new(bot.clone()));
    let tick_interval = config.tick_interval;
    tokio::spawn(async move { scheduler.run(tick_interval).await });

    // Telegram keeps the command menu server-side, per bot token. Without this it keeps
    // showing whatever was registered last — including commands from an entirely different
    // program that once held this token. Re-registering on every start also means adding a
    // variant to `TelegramCommand` is all it takes for the menu to follow.
    if let Err(error) = bot.set_my_commands(TelegramCommand::bot_commands()).await {
        // Not fatal: the bot answers its commands whether or not the menu lists them, and
        // refusing to start over a cosmetic call would be worse than a stale menu.
        tracing::warn!(%error, "could not register the command menu with Telegram");
    }

    tracing::info!(
        allowed = telegram.allowed_users.len(),
        model = %config.model,
        timezone = %config.timezone,
        "serana is up"
    );

    let state = Arc::new(AppState {
        reminders: wiring.reminders,
        telegram,
    });
    // Two branches over the same messages: a slash command, or anything else. Without the
    // second, an answer to a question the assistant just asked ("tomorrow at 9") would be
    // silently dropped, which is the one thing a conversation cannot afford.
    let handler = Update::filter_message()
        .branch(
            dptree::entry()
                .filter_command::<TelegramCommand>()
                .endpoint(dispatch),
        )
        .branch(dptree::endpoint(dispatch_text));
    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![state])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    Ok(())
}

/// Send `body` back, logging rather than failing the update if it cannot be delivered.
async fn reply(bot: &Bot, message: &Message, body: String) {
    if let Err(error) = bot.send_message(message.chat.id, body).await {
        tracing::warn!(%error, "could not send a reply");
    }
}

/// A message that is not a command: treated as a continuation of the conversation.
async fn dispatch_text(bot: Bot, message: Message, state: Arc<AppState>) -> anyhow::Result<()> {
    let Some(text) = message
        .text()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
    else {
        // Stickers, photos, joins — nothing to read.
        return Ok(());
    };
    // A slash-prefixed word that reached here is a command we do not have; answering it as
    // a reminder request would be worse than saying so.
    if text.starts_with('/') {
        reply(&bot, &message, text::UNKNOWN_COMMAND.to_owned()).await;
        return Ok(());
    }
    answer(bot, message, Command::Reminder(text), state).await
}

async fn dispatch(
    bot: Bot,
    message: Message,
    command: TelegramCommand,
    state: Arc<AppState>,
) -> anyhow::Result<()> {
    answer(bot, message, Command::from(command), state).await
}

/// Check the sender, run the command, send the reply.
async fn answer(
    bot: Bot,
    message: Message,
    command: Command,
    state: Arc<AppState>,
) -> anyhow::Result<()> {
    let Some(sender) = message
        .from
        .as_ref()
        .map(|user| UserId::new(user.id.0 as i64))
    else {
        // Channel posts and similar have no sender; there is nobody to answer.
        return Ok(());
    };

    let reply = if state.telegram.allows(sender) {
        respond(
            &state.reminders,
            sender,
            &ConversationId::new(format!("tg:{sender}")),
            &command,
        )
        .await
    } else {
        // Logged at warn so an owner who mistyped their own id can see why nothing works.
        tracing::warn!(user = %sender, "refused a command from a user outside the allowlist");
        text::NOT_ALLOWED.to_owned()
    };

    bot.send_message(message.chat.id, reply).await?;
    Ok(())
}
