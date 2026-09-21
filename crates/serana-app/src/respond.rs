//! Turning a [`Command`] into the words to send back.
//!
//! No transport, no `Bot`, no terminal — just a service and a string. That is what makes
//! the command surface testable: the tests below drive the real service against fakes and
//! assert on exactly what the user would read.

use serana_domain::reminder::{ReminderRepository, UserId};
use serana_domain::{Clock, IdGenerator, LlmProvider};
use serana_services::{ReminderError, ReminderService};

use crate::{Command, text};

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
        Command::Help => text::HELP.to_owned(),

        Command::Reminder(request) if request.trim().is_empty() => {
            text::REMINDER_NEEDS_TEXT.to_owned()
        }
        Command::Reminder(request) => match service.handle(user, request).await {
            Ok(outcome) => text::outcome(&outcome),
            Err(error) => render_error(&error),
        },

        Command::Reminders => match service.list(user).await {
            Ok(reminders) => text::listing(&reminders),
            Err(error) => render_error(&error),
        },
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
        ReminderError::Unparsable(reason) => format!("I did not understand: {reason}"),
        ReminderError::NeverFires => "That schedule would never fire — check the date.".to_owned(),
        ReminderError::NotFound(id) => {
            format!("No reminder with id {id}. See yours with /reminders")
        }
        ReminderError::Llm(error) => {
            tracing::warn!(%error, "model call failed");
            "The model is unavailable right now. Try again in a minute.".to_owned()
        }
        ReminderError::Storage(error) => {
            tracing::error!(%error, "storage failed");
            "Could not save that. Try again.".to_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serana_domain::message::ToolCall;
    use serana_domain::reminder::TimeZoneName;
    use serana_services::ReminderConfig;
    use serana_testkit::{FixedClock, InMemoryReminderRepository, ScriptedLlm, SequentialIds};

    use super::*;
    use crate::text;

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
                temperature: None,
            },
        );
        (service, llm)
    }

    fn service(llm: ScriptedLlm) -> Service {
        service_with_llm(llm).0
    }

    fn extracting(times: usize) -> ScriptedLlm {
        let arguments = serde_json::json!({
            "kind": "monthly", "time": "10:00", "days_of_month": [20], "text": "оформить invoice"
        })
        .to_string();
        (0..times).fold(ScriptedLlm::new(), |llm, index| {
            llm.calling(vec![ToolCall::new(
                format!("call_{index}"),
                "create_reminder",
                arguments.clone(),
            )])
        })
    }

    #[tokio::test]
    async fn help_explains_the_commands() {
        let service = service(ScriptedLlm::new());
        let reply = respond(&service, OWNER, &Command::Help).await;
        assert!(reply.contains("/reminder"), "{reply}");
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
        assert!(
            reply.contains("every month on the 20th at 10:00"),
            "{reply}"
        );
        assert!(reply.contains("20 March 2026 at 10:00"), "{reply}");
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
    async fn an_id_the_model_padded_with_whitespace_still_resolves() {
        let service = service(create_then(
            "delete_reminder",
            serde_json::json!({ "id": "  r1  " }),
        ));
        respond(&service, OWNER, &Command::Reminder("что-то".into())).await;

        let reply = respond(&service, OWNER, &Command::Reminder("remove it".into())).await;
        assert!(reply.contains("Deleted"), "{reply}");
        assert_eq!(
            respond(&service, OWNER, &Command::Reminders).await,
            text::NO_REMINDERS
        );
    }

    /// A model that calls `tool` with `arguments`, once per element.
    fn calling(tool: &'static str, arguments: Vec<serde_json::Value>) -> ScriptedLlm {
        arguments
            .into_iter()
            .enumerate()
            .fold(ScriptedLlm::new(), |llm, (index, args)| {
                llm.calling(vec![ToolCall::new(
                    format!("call_{index}"),
                    tool,
                    args.to_string(),
                )])
            })
    }

    /// A model that creates once, then does `then` with `args`.
    fn create_then(then: &'static str, args: serde_json::Value) -> ScriptedLlm {
        ScriptedLlm::new()
            .calling(vec![ToolCall::new(
                "call_0",
                "create_reminder",
                serde_json::json!({
                    "kind": "monthly", "time": "10:00", "days_of_month": [20],
                    "text": "оформить invoice"
                })
                .to_string(),
            )])
            .calling(vec![ToolCall::new("call_1", then, args.to_string())])
    }

    /// The id `SequentialIds` hands the first reminder.
    const FIRST_ID: &str = "r1";

    #[tokio::test]
    async fn the_model_chooses_the_verb_so_one_command_covers_deleting() {
        let service = service(create_then(
            "delete_reminder",
            serde_json::json!({ "id": FIRST_ID }),
        ));
        respond(
            &service,
            OWNER,
            &Command::Reminder("каждый месяц 20".into()),
        )
        .await;

        let reply = respond(
            &service,
            OWNER,
            &Command::Reminder("remove the invoice reminder".into()),
        )
        .await;
        assert!(reply.contains("Deleted"), "{reply}");
        assert!(reply.contains("оформить invoice"), "{reply}");
        assert!(service.list(OWNER).await.unwrap().is_empty(), "it is gone");
    }

    #[tokio::test]
    async fn the_model_can_acknowledge_rather_than_delete() {
        let service = service(create_then(
            "acknowledge_reminder",
            serde_json::json!({ "id": FIRST_ID }),
        ));
        respond(
            &service,
            OWNER,
            &Command::Reminder("каждый месяц 20".into()),
        )
        .await;

        let reply = respond(
            &service,
            OWNER,
            &Command::Reminder("already sent the invoice".into()),
        )
        .await;
        assert!(reply.contains("Done"), "{reply}");
        assert!(reply.contains("Back on"), "{reply}");
        assert_eq!(
            service.list(OWNER).await.unwrap().len(),
            1,
            "acknowledging keeps the reminder"
        );
    }

    #[tokio::test]
    async fn the_model_can_change_an_existing_reminder() {
        let service = service(create_then(
            "update_reminder",
            serde_json::json!({
                "id": FIRST_ID, "kind": "monthly", "time": "22:30",
                "days_of_month": [20, 21, 22, 23, 24, 25, 26],
                "text": "оформить invoice"
            }),
        ));
        respond(
            &service,
            OWNER,
            &Command::Reminder("каждый месяц 20".into()),
        )
        .await;

        let reply = respond(
            &service,
            OWNER,
            &Command::Reminder("make it the 20th to the 26th at 22:30".into()),
        )
        .await;
        assert!(reply.contains("Updated"), "{reply}");
        assert!(
            reply.contains("every month on the 20th to the 26th at 22:30"),
            "{reply}"
        );
        let stored = service.list(OWNER).await.unwrap();
        assert_eq!(stored.len(), 1, "changed, not duplicated");
        assert_eq!(stored[0].id.as_str(), FIRST_ID, "same reminder");
    }

    #[tokio::test]
    async fn an_ambiguous_request_comes_back_as_the_models_own_question() {
        // Stage one has no buttons: the model is told to ask rather than guess, and its
        // question reaches the user verbatim because it names the candidates.
        let question = "I found two invoice reminders — the salary one or the contractor one?";
        let service = service(ScriptedLlm::new().answering(question));
        let reply = respond(
            &service,
            OWNER,
            &Command::Reminder("remove the invoice reminder".into()),
        )
        .await;
        assert_eq!(reply, question);
    }

    #[tokio::test]
    async fn acting_on_someone_elses_reminder_reports_not_found() {
        let service = service(create_then(
            "delete_reminder",
            serde_json::json!({ "id": FIRST_ID }),
        ));
        respond(
            &service,
            OWNER,
            &Command::Reminder("каждый месяц 20".into()),
        )
        .await;

        let reply = respond(&service, OTHER, &Command::Reminder("remove it".into())).await;
        assert!(reply.contains("No reminder with id"), "{reply}");
        assert_eq!(
            service.list(OWNER).await.unwrap().len(),
            1,
            "it is left alone"
        );
    }

    #[tokio::test]
    async fn an_id_the_model_invented_is_refused() {
        // The only ids it may use are the ones the prompt gave it.
        let service = service(calling(
            "delete_reminder",
            vec![serde_json::json!({ "id": "made-up" })],
        ));
        let reply = respond(
            &service,
            OWNER,
            &Command::Reminder("remove something".into()),
        )
        .await;
        assert!(reply.contains("No reminder with id"), "{reply}");
    }

    #[tokio::test]
    async fn a_tool_name_we_do_not_offer_is_not_dispatched() {
        let service = service(calling(
            "drop_database",
            vec![serde_json::json!({ "id": "r1" })],
        ));
        let reply = respond(&service, OWNER, &Command::Reminder("do something".into())).await;
        // Falls through to prose rather than being treated as an action.
        assert!(!reply.contains("Deleted"), "{reply}");
    }
}
