//! Building the object graph.
//!
//! Both binaries need the same database, the same provider and the same service, wired the
//! same way. Doing it twice would mean the CLI and the bot drifting onto different models
//! or different files without anyone noticing.

use std::sync::Arc;

use anyhow::Context;
use serana_adapters::{
    OpenAiConfig, OpenAiProvider, RandomIds, SqliteConversationRepository,
    SqliteReminderRepository, SystemClock,
};
use serana_domain::reminder::Notifier;
use serana_services::{
    DEFAULT_COMPACT_ABOVE_TOKENS, ReminderConfig, ReminderService, SchedulerService,
};

use crate::AppConfig;

/// The reminder service, with every port resolved to its real implementation.
pub type Reminders = ReminderService<
    SqliteReminderRepository,
    Arc<OpenAiProvider>,
    SystemClock,
    RandomIds,
    SqliteConversationRepository,
>;

/// What a frontend gets after wiring.
pub struct Wiring {
    /// Kept so the frontend can build a scheduler over the same database.
    pub repository: SqliteReminderRepository,
    pub reminders: Reminders,
}

/// Open the database and assemble the services.
///
/// Creates the data directory if it is missing, so a fresh volume works without setup.
pub async fn build_reminders(config: &AppConfig) -> anyhow::Result<Wiring> {
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

    let conversations = SqliteConversationRepository::open(config.database_path())
        .await
        .with_context(|| format!("could not open {}", config.database_path().display()))?;

    let reminders = ReminderService::new(
        repository.clone(),
        provider,
        SystemClock,
        RandomIds,
        conversations,
        ReminderConfig {
            model: config.model.clone(),
            summary_model: config.auxiliary_model.clone(),
            default_timezone: config.timezone.clone(),
            temperature: config.temperature,
            compact_above_tokens: DEFAULT_COMPACT_ABOVE_TOKENS,
        },
    );

    Ok(Wiring {
        repository,
        reminders,
    })
}

/// A scheduler over the same database, delivering through `notifier`.
///
/// The notifier is the one part a frontend supplies itself: the bot pushes to Telegram, and
/// a terminal has nowhere to push to.
pub fn build_scheduler<N: Notifier>(
    repository: SqliteReminderRepository,
    notifier: N,
) -> SchedulerService<SqliteReminderRepository, N, SystemClock> {
    SchedulerService::new(repository, notifier, SystemClock)
}
