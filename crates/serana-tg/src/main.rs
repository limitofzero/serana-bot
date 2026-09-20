//! The Telegram frontend: a composition root and a dispatcher, and nothing else.
//!
//! Anything resembling a decision belongs in `serana-services`, where it can be tested
//! without a bot token.

use std::sync::Arc;

use anyhow::Context;
use serana_adapters::{
    OpenAiConfig, OpenAiProvider, RandomIds, SqliteReminderRepository, SystemClock,
};
use serana_domain::reminder::UserId;
use serana_services::{ReminderConfig, ReminderService, SchedulerService};
use teloxide::prelude::*;

use serana_tg::commands::{self, Command};
use serana_tg::config::Config;
use serana_tg::notifier::TelegramNotifier;
use serana_tg::text;

/// The wired object graph, concrete at last.
type Reminders =
    ReminderService<SqliteReminderRepository, Arc<OpenAiProvider>, SystemClock, RandomIds>;

struct AppState {
    reminders: Reminders,
    config: Config,
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

    let config = Config::from_env()?;
    if config.allowed_users.is_empty() {
        tracing::warn!(
            "SERANA_ALLOWED_USER_IDS is empty, so the bot will answer nobody. \
             Set it to your Telegram user id."
        );
    }

    tokio::fs::create_dir_all(&config.data_dir)
        .await
        .with_context(|| format!("could not create {}", config.data_dir.display()))?;
    let repository = SqliteReminderRepository::open(config.database_path())
        .await
        .with_context(|| format!("could not open {}", config.database_path().display()))?;

    let provider = Arc::new(OpenAiProvider::new(OpenAiConfig::new(
        &config.base_url,
        &config.api_key,
    ))?);

    let bot = Bot::new(&config.telegram_token);

    // One scheduler, ticking beside the dispatcher. Both share the repository; SQLite in
    // WAL mode lets the tick read while a command writes.
    let scheduler = SchedulerService::new(
        repository.clone(),
        TelegramNotifier::new(bot.clone()),
        SystemClock,
    );
    let tick_interval = config.tick_interval;
    tokio::spawn(async move { scheduler.run(tick_interval).await });

    let state = Arc::new(AppState {
        reminders: ReminderService::new(
            repository,
            provider,
            SystemClock,
            RandomIds,
            ReminderConfig {
                model: config.model.clone(),
                default_timezone: config.timezone.clone(),
            },
        ),
        config,
    });

    tracing::info!(
        allowed = state.config.allowed_users.len(),
        model = %state.config.model,
        timezone = %state.config.timezone,
        "serana is up"
    );

    let handler = Update::filter_message()
        .filter_command::<Command>()
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

    let reply = if state.config.allows(sender) {
        commands::respond(&state.reminders, sender, &command).await
    } else {
        // Logged at warn so an owner who mistyped their own id can see why nothing works.
        tracing::warn!(user = %sender, "refused a command from a user outside the allowlist");
        text::NOT_ALLOWED.to_owned()
    };

    bot.send_message(message.chat.id, reply).await?;
    Ok(())
}
