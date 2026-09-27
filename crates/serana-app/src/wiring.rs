//! Building the object graph.
//!
//! Both binaries need the same database, the same provider and the same service, wired the
//! same way. Doing it twice would mean the CLI and the bot drifting onto different models
//! or different files without anyone noticing.

use std::sync::Arc;

use anyhow::Context;
use serana_adapters::{
    GoogleCalendar, OpenAiConfig, OpenAiProvider, RandomIds, SqliteConversationRepository,
    SqliteReminderRepository, SystemClock,
};
use serana_domain::reminder::Notifier;
use serana_services::{
    CalendarChat, CalendarService, Chat, ChatConfig, ChatService, DEFAULT_COMPACT_ABOVE_TOKENS,
    Planning, ReminderService, SchedulerService,
};

use crate::AppConfig;

/// The reminder service, with every port resolved to its real implementation.
pub type Reminders = ChatService<
    SqliteReminderRepository,
    Arc<OpenAiProvider>,
    SystemClock,
    RandomIds,
    SqliteConversationRepository,
>;

/// The calendar conversation, with every port resolved.
pub type Calendar =
    CalendarChat<GoogleCalendar, Arc<OpenAiProvider>, SystemClock, SqliteConversationRepository>;

/// What a frontend gets after wiring.
pub struct Wiring {
    /// Kept so the frontend can build a scheduler over the same database.
    pub repository: SqliteReminderRepository,
    pub reminders: Reminders,
    /// `None` when no calendar is configured, which is a supported way to run.
    pub calendar: Option<Calendar>,
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

    let chat_config = ChatConfig {
        model: config.model.clone(),
        summary_model: config.auxiliary_model.clone(),
        default_timezone: config.timezone.clone(),
        temperature: config.temperature,
        compact_above_tokens: DEFAULT_COMPACT_ABOVE_TOKENS,
    };

    let reminders = ChatService::new(
        ReminderService::new(repository.clone(), SystemClock, RandomIds),
        Arc::clone(&provider),
        conversations.clone(),
        chat_config.clone(),
    );

    Ok(Wiring {
        repository,
        reminders,
        calendar: build_calendar(config, provider, conversations, chat_config),
    })
}

/// The calendar conversation, if there is a calendar to have one about.
///
/// A client that will not build is logged and dropped rather than failing the start: it
/// costs the calendar, and refusing to run the reminders over it would be a worse trade
/// than saying no calendar is connected.
fn build_calendar(
    config: &AppConfig,
    provider: Arc<OpenAiProvider>,
    conversations: SqliteConversationRepository,
    chat_config: ChatConfig,
) -> Option<Calendar> {
    let google = config.calendar.clone()?;
    let calendar = match GoogleCalendar::new(google) {
        Ok(calendar) => calendar,
        Err(error) => {
            tracing::warn!(%error, "could not build the Google Calendar client");
            return None;
        }
    };
    Some(Chat::new(
        Planning::new(CalendarService::new(
            calendar,
            SystemClock,
            config.timezone.clone(),
            config.travel,
        )),
        provider,
        conversations,
        chat_config,
    ))
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
