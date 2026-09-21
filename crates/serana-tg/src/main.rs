//! The Telegram bot: read the environment, wire the graph, dispatch.

use std::sync::Arc;

use serana_app::{AppConfig, Command, Reminders, build_reminders, build_scheduler, respond, text};
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
    let handler = Update::filter_message()
        .filter_command::<TelegramCommand>()
        .endpoint(dispatch);
    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![state])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    Ok(())
}

async fn dispatch(
    bot: Bot,
    message: Message,
    command: TelegramCommand,
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
        respond(&state.reminders, sender, &Command::from(command)).await
    } else {
        // Logged at warn so an owner who mistyped their own id can see why nothing works.
        tracing::warn!(user = %sender, "refused a command from a user outside the allowlist");
        text::NOT_ALLOWED.to_owned()
    };

    bot.send_message(message.chat.id, reply).await?;
    Ok(())
}
