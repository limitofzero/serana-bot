//! Command handling, kept independent of Telegram.
//!
//! [`respond`] turns a command into the text to send back and nothing else — no `Bot`, no
//! network. That is what makes the command surface testable: the tests below drive the real
//! service against fakes and assert on what the user would actually read.

use serana_domain::reminder::{ReminderId, ReminderRepository, UserId};
use serana_domain::{Clock, IdGenerator, LlmProvider};
use serana_services::{ReminderError, ReminderService};
use teloxide::utils::command::BotCommands;

use crate::text;

#[derive(BotCommands, Clone, Debug, PartialEq, Eq)]
#[command(rename_rule = "snake_case")]
pub enum Command {
    /// Show what this bot can do.
    Help,
    /// Same as /help, sent automatically when a chat opens.
    Start,
    /// Set a reminder, described however you like.
    Reminder(String),
    /// List your reminders.
    Reminders,
    /// Delete a reminder by id.
    ReminderDelete(String),
}

/// Produce the reply for `command`, issued by `user`.
pub async fn respond<R, L, C, I>(
    service: &ReminderService<R, L, C, I>,
    user: UserId,
    command: &Command,
) -> String
where
    R: ReminderRepository,
    L: LlmProvider,
    C: Clock,
    I: IdGenerator,
{
    match command {
        Command::Help | Command::Start => text::HELP.to_owned(),

        Command::Reminder(request) if request.trim().is_empty() => {
            text::REMINDER_NEEDS_TEXT.to_owned()
        }
        Command::Reminder(request) => match service.create(user, request).await {
            Ok(reminder) => text::created(&reminder),
            Err(error) => render_error(&error),
        },

        Command::Reminders => match service.list(user).await {
            Ok(reminders) => text::listing(&reminders),
            Err(error) => render_error(&error),
        },

        Command::ReminderDelete(id) if id.trim().is_empty() => text::DELETE_NEEDS_ID.to_owned(),
        Command::ReminderDelete(id) => {
            match service.delete(user, &ReminderId::new(id.trim())).await {
                Ok(reminder) => text::deleted(&reminder),
                Err(error) => render_error(&error),
            }
        }
    }
}

/// Turn a failure into something worth reading.
///
/// Parse failures carry the model's own explanation and are shown verbatim, because they
/// tell the user what to rephrase. Provider and storage failures are not the user's problem
/// and their detail goes to the log instead — but they are still distinguished, so nobody
/// is told their phrasing was bad when the API was simply down.
fn render_error(error: &ReminderError) -> String {
    match error {
        ReminderError::Unparsable(reason) => format!("Не понял: {reason}"),
        ReminderError::NeverFires => {
            "Такое расписание никогда не сработает — проверьте дату.".to_owned()
        }
        ReminderError::NotFound(id) => {
            format!("Нет напоминания с id {id}. Посмотреть свои — /reminders")
        }
        ReminderError::Llm(error) => {
            tracing::warn!(%error, "model call failed");
            "Модель сейчас недоступна. Попробуйте ещё раз через минуту.".to_owned()
        }
        ReminderError::Storage(error) => {
            tracing::error!(%error, "storage failed");
            "Не смог сохранить. Попробуйте ещё раз.".to_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serana_domain::LlmError;
    use serana_domain::message::ToolCall;
    use serana_domain::reminder::TimeZoneName;
    use serana_services::ReminderConfig;
    use serana_testkit::{FixedClock, InMemoryReminderRepository, ScriptedLlm, SequentialIds};

    use super::*;

    const OWNER: UserId = UserId::new(42);
    const OTHER: UserId = UserId::new(7);

    type Service =
        ReminderService<InMemoryReminderRepository, Arc<ScriptedLlm>, FixedClock, SequentialIds>;

    /// Returns the service and a handle on its scripted model, so a test can assert that
    /// a command did *not* reach the provider.
    fn service_with_llm(llm: ScriptedLlm) -> (Service, Arc<ScriptedLlm>) {
        let llm = Arc::new(llm);
        let service = ReminderService::new(
            InMemoryReminderRepository::new(),
            Arc::clone(&llm),
            FixedClock::at("2026-03-10T06:00:00Z"),
            SequentialIds::default(),
            ReminderConfig {
                model: "gpt-5-mini".into(),
                default_timezone: TimeZoneName::new("Asia/Tbilisi"),
            },
        );
        (service, llm)
    }

    fn service(llm: ScriptedLlm) -> Service {
        service_with_llm(llm).0
    }

    fn extracting(times: usize) -> ScriptedLlm {
        let arguments = serde_json::json!({
            "kind": "monthly", "time": "10:00", "day_of_month": 20, "text": "оформить invoice"
        })
        .to_string();
        (0..times).fold(ScriptedLlm::new(), |llm, index| {
            llm.calling(vec![ToolCall::new(
                format!("call_{index}"),
                "schedule_reminder",
                arguments.clone(),
            )])
        })
    }

    #[tokio::test]
    async fn help_and_start_both_explain_the_bot() {
        let service = service(ScriptedLlm::new());
        for command in [Command::Help, Command::Start] {
            let reply = respond(&service, OWNER, &command).await;
            assert!(reply.contains("/reminder"), "{reply}");
        }
    }

    #[tokio::test]
    async fn the_request_from_the_brief_is_confirmed_back_in_words() {
        let service = service(extracting(1));
        let reply = respond(
            &service,
            OWNER,
            &Command::Reminder(
                "каждый месяц 20 число - писать мне что надо оформить invoice".into(),
            ),
        )
        .await;

        assert!(reply.contains("оформить invoice"), "{reply}");
        assert!(reply.contains("каждое 20 число в 10:00"), "{reply}");
        assert!(reply.contains("20 марта 2026 в 10:00"), "{reply}");
    }

    #[tokio::test]
    async fn a_bare_reminder_command_asks_for_the_missing_text_without_calling_the_model() {
        let (service, llm) = service_with_llm(ScriptedLlm::new());
        for argument in ["", "   "] {
            let reply = respond(&service, OWNER, &Command::Reminder(argument.into())).await;
            assert_eq!(reply, text::REMINDER_NEEDS_TEXT);
        }
        assert_eq!(llm.call_count(), 0, "no tokens spent on an empty command");
    }

    #[tokio::test]
    async fn an_empty_list_invites_the_user_to_create_one() {
        let service = service(ScriptedLlm::new());
        assert_eq!(
            respond(&service, OWNER, &Command::Reminders).await,
            text::NO_REMINDERS
        );
    }

    #[tokio::test]
    async fn a_listing_shows_only_your_own_reminders() {
        let service = service(extracting(2));
        respond(&service, OWNER, &Command::Reminder("мне".into())).await;
        respond(&service, OTHER, &Command::Reminder("им".into())).await;

        let reply = respond(&service, OWNER, &Command::Reminders).await;
        assert_eq!(reply.matches("id: ").count(), 1, "{reply}");
        assert!(reply.contains("id: r1"), "{reply}");
    }

    #[tokio::test]
    async fn a_reminder_can_be_deleted_by_the_id_the_listing_showed() {
        let service = service(extracting(1));
        respond(&service, OWNER, &Command::Reminder("что-то".into())).await;

        let reply = respond(&service, OWNER, &Command::ReminderDelete("r1".into())).await;
        assert!(reply.contains("Удалил"), "{reply}");
        assert_eq!(
            respond(&service, OWNER, &Command::Reminders).await,
            text::NO_REMINDERS
        );
    }

    #[tokio::test]
    async fn an_id_with_stray_whitespace_still_deletes() {
        let service = service(extracting(1));
        respond(&service, OWNER, &Command::Reminder("что-то".into())).await;
        let reply = respond(&service, OWNER, &Command::ReminderDelete("  r1  ".into())).await;
        assert!(reply.contains("Удалил"), "{reply}");
    }

    #[tokio::test]
    async fn deleting_without_an_id_explains_where_to_find_one() {
        let service = service(ScriptedLlm::new());
        let reply = respond(&service, OWNER, &Command::ReminderDelete("".into())).await;
        assert_eq!(reply, text::DELETE_NEEDS_ID);
    }

    #[tokio::test]
    async fn deleting_someone_elses_reminder_looks_exactly_like_a_missing_one() {
        let service = service(extracting(1));
        respond(&service, OWNER, &Command::Reminder("что-то".into())).await;

        let theirs = respond(&service, OTHER, &Command::ReminderDelete("r1".into())).await;
        let missing = respond(&service, OTHER, &Command::ReminderDelete("zzzz".into())).await;
        assert!(theirs.contains("Нет напоминания"), "{theirs}");
        assert!(missing.contains("Нет напоминания"), "{missing}");

        // And it is still there for its owner.
        assert!(
            respond(&service, OWNER, &Command::Reminders)
                .await
                .contains("id: r1")
        );
    }

    #[tokio::test]
    async fn an_unreadable_request_repeats_the_models_explanation() {
        let service = service(ScriptedLlm::new().answering("Не указано время суток."));
        let reply = respond(&service, OWNER, &Command::Reminder("напомни".into())).await;
        assert!(reply.contains("Не указано время суток."), "{reply}");
    }

    #[tokio::test]
    async fn a_provider_outage_is_not_blamed_on_the_users_phrasing() {
        let service = service(ScriptedLlm::new().failing(LlmError::Auth("bad key".into())));
        let reply = respond(
            &service,
            OWNER,
            &Command::Reminder("каждый день в 10".into()),
        )
        .await;
        assert!(reply.contains("Модель сейчас недоступна"), "{reply}");
        assert!(
            !reply.contains("bad key"),
            "provider detail must not leak to the user: {reply}"
        );
    }

    #[tokio::test]
    async fn a_schedule_that_never_fires_says_so_plainly() {
        let service = service(ScriptedLlm::new().calling(vec![ToolCall::new(
            "c1",
            "schedule_reminder",
            serde_json::json!({"kind": "once", "time": "10:00", "date": "2020-01-01",
                               "text": "прошлое"})
            .to_string(),
        )]));
        let reply = respond(&service, OWNER, &Command::Reminder("1 января 2020".into())).await;
        assert!(reply.contains("никогда не сработает"), "{reply}");
    }
}
