//! Turning a [`Command`] into the words to send back.
//!
//! No transport, no `Bot`, no terminal — just the services and a string. That is what makes
//! the command surface testable: the tests below drive the real services against fakes and
//! assert on exactly what the user would read.

use serana_domain::calendar::{CalendarError, CalendarPort};
use serana_domain::conversation::ConversationId;
use serana_domain::conversation_store::ConversationRepository;
use serana_domain::reminder::{ReminderRepository, UserId};
use serana_domain::{Clock, IdGenerator, LlmProvider};
use serana_services::{CalendarChat, CalendarTurnError, ChatService, ReminderError};

use crate::{Command, text};

/// Produce the reply for `command`, issued by `user`.
///
/// `calendar` is `None` when none is configured, which is not a failure: the assistant
/// simply has no calendar, and says so rather than apologising for an outage.
pub async fn respond<R, L, C, I, V, P>(
    reminders: &ChatService<R, L, C, I, V>,
    calendar: Option<&CalendarChat<P, L, C, V>>,
    user: UserId,
    conversation: &ConversationId,
    command: &Command,
) -> String
where
    R: ReminderRepository,
    L: LlmProvider,
    C: Clock,
    I: IdGenerator,
    V: ConversationRepository,
    P: CalendarPort,
{
    match command {
        Command::Help => text::HELP.to_owned(),

        Command::Reminder(request) if request.trim().is_empty() => {
            text::reminders::REMINDER_NEEDS_TEXT.to_owned()
        }
        Command::Reminder(request) => match reminders.handle(user, conversation, request).await {
            Ok(outcome) => text::reminders::outcome(&outcome),
            Err(error) => render_error(&error),
        },

        Command::Reminders => match reminders.reminders().list(user).await {
            Ok(list) => text::reminders::listing(&list, reminders.reminders().now()),
            Err(error) => render_error(&error),
        },

        Command::Calendar(_) if calendar.is_none() => text::calendar::NOT_CONNECTED.to_owned(),
        Command::Calendar(request) if request.trim().is_empty() => {
            text::calendar::CALENDAR_NEEDS_TEXT.to_owned()
        }
        Command::Calendar(request) => {
            let Some(calendar) = calendar else {
                unreachable!("the arm above answers when there is none")
            };
            let zone = calendar.config().default_timezone.clone();
            match calendar.handle(user, conversation, request).await {
                Ok(outcome) => text::calendar::outcome(&outcome, &zone),
                Err(error) => render_calendar_error(&error),
            }
        }

        // Both conversations, because the person asked to fold up "this" and has no way to
        // know the assistant keeps two. Either having had something to say is enough.
        Command::Compact => {
            let folded = match reminders.compact_conversation(conversation).await {
                Ok(folded) => folded,
                Err(error) => return render_error(&error),
            };
            let folded = match calendar {
                None => folded,
                Some(calendar) => match calendar.compact_conversation(conversation).await {
                    Ok(also) => folded || also,
                    Err(error) => return render_calendar_error(&error),
                },
            };
            if folded {
                text::COMPACTED.to_owned()
            } else {
                text::NOTHING_TO_COMPACT.to_owned()
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
        ReminderError::Unparsable(reason) => format!("I did not understand: {reason}"),
        ReminderError::NeverFires => "That schedule would never fire — check the date.".to_owned(),
        ReminderError::NoChecklist(id) => {
            format!("Reminder {id} is a plain reminder — there is no checklist to tick.")
        }
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

/// The same, for the calendar.
///
/// "No calendar is connected" is deliberately not an apology: nothing has gone wrong, there
/// is simply nothing to talk to.
fn render_calendar_error(error: &CalendarTurnError) -> String {
    match error {
        CalendarTurnError::Unparsable(reason) => format!("I did not understand: {reason}"),
        CalendarTurnError::NotFound(id) => {
            format!("Event {id} is not on your calendar any more.")
        }
        CalendarTurnError::Calendar(CalendarError::NotConnected) => {
            text::calendar::NOT_CONNECTED.to_owned()
        }
        CalendarTurnError::Calendar(error) => {
            tracing::warn!(%error, "calendar call failed");
            "Your calendar is not answering right now. Try again in a minute.".to_owned()
        }
        CalendarTurnError::Llm(error) => {
            tracing::warn!(%error, "model call failed");
            "The model is unavailable right now. Try again in a minute.".to_owned()
        }
        CalendarTurnError::Storage(error) => {
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
    use serana_services::{ChatConfig, DEFAULT_COMPACT_ABOVE_TOKENS, ReminderService};
    use serana_testkit::{
        FixedClock, InMemoryCalendar, InMemoryConversationRepository, InMemoryReminderRepository,
        ScriptedLlm, SequentialIds,
    };

    use super::*;
    use crate::text;

    pub(super) fn chat() -> ConversationId {
        ConversationId::new("test")
    }

    pub(super) const OWNER: UserId = UserId::new(42);
    const OTHER: UserId = UserId::new(7);

    type Service = ChatService<
        InMemoryReminderRepository,
        Arc<ScriptedLlm>,
        FixedClock,
        SequentialIds,
        InMemoryConversationRepository,
    >;

    /// 10:00 on a Tuesday, in Tbilisi.
    pub(super) const NOW: &str = "2026-03-10T06:00:00Z";
    pub(super) const TRAVEL: std::time::Duration = std::time::Duration::from_secs(30 * 60);

    pub(super) fn chat_config() -> ChatConfig {
        ChatConfig {
            model: "gpt-5-mini".into(),
            default_timezone: TimeZoneName::new("Asia/Tbilisi"),
            temperature: None,
            summary_model: None,
            compact_above_tokens: DEFAULT_COMPACT_ABOVE_TOKENS,
        }
    }

    /// Returns the service and a handle on its scripted model, so a test can assert that
    /// a command did *not* reach the provider.
    pub(super) fn service_with_llm(llm: ScriptedLlm) -> (Service, Arc<ScriptedLlm>) {
        let llm = Arc::new(llm);
        let service = ChatService::new(
            ReminderService::new(
                InMemoryReminderRepository::new(),
                FixedClock::at(NOW),
                SequentialIds::default(),
            ),
            Arc::clone(&llm),
            InMemoryConversationRepository::new(),
            chat_config(),
        );
        (service, llm)
    }

    pub(super) fn service(llm: ScriptedLlm) -> Service {
        service_with_llm(llm).0
    }

    pub(super) type Calendar = CalendarChat<
        InMemoryCalendar,
        Arc<ScriptedLlm>,
        FixedClock,
        InMemoryConversationRepository,
    >;

    /// Most of these are about reminders, and a deployment with no calendar configured is a
    /// real one.
    fn no_calendar() -> Option<&'static Calendar> {
        None
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
        let reply = respond(&service, no_calendar(), OWNER, &chat(), &Command::Help).await;
        assert!(reply.contains("/reminder"), "{reply}");
    }

    #[tokio::test]
    async fn the_request_from_the_brief_is_confirmed_back_in_words() {
        let service = service(extracting(1));
        let reply = respond(
            &service,
            no_calendar(),
            OWNER,
            &chat(),
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
            let reply = respond(
                &service,
                no_calendar(),
                OWNER,
                &chat(),
                &Command::Reminder(argument.into()),
            )
            .await;
            assert_eq!(reply, text::reminders::REMINDER_NEEDS_TEXT);
        }
        assert_eq!(llm.call_count(), 0, "no tokens spent on an empty command");
    }

    #[tokio::test]
    async fn an_empty_list_invites_the_user_to_create_one() {
        let service = service(ScriptedLlm::new());
        assert_eq!(
            respond(&service, no_calendar(), OWNER, &chat(), &Command::Reminders).await,
            text::reminders::NO_REMINDERS
        );
    }

    #[tokio::test]
    async fn a_listing_shows_only_your_own_reminders() {
        let service = service(extracting(2));
        respond(
            &service,
            no_calendar(),
            OWNER,
            &chat(),
            &Command::Reminder("мне".into()),
        )
        .await;
        respond(
            &service,
            no_calendar(),
            OTHER,
            &chat(),
            &Command::Reminder("им".into()),
        )
        .await;

        let reply = respond(&service, no_calendar(), OWNER, &chat(), &Command::Reminders).await;
        assert_eq!(reply.matches("🆔 ").count(), 1, "{reply}");
        assert!(reply.contains("🆔 r1"), "{reply}");
    }

    #[tokio::test]
    async fn an_id_the_model_padded_with_whitespace_still_resolves() {
        let service = service(create_then(
            "delete_reminder",
            serde_json::json!({ "id": "  r1  " }),
        ));
        respond(
            &service,
            no_calendar(),
            OWNER,
            &chat(),
            &Command::Reminder("что-то".into()),
        )
        .await;

        let reply = respond(
            &service,
            no_calendar(),
            OWNER,
            &chat(),
            &Command::Reminder("remove it".into()),
        )
        .await;
        assert!(reply.contains("Deleted"), "{reply}");
        assert_eq!(
            respond(&service, no_calendar(), OWNER, &chat(), &Command::Reminders).await,
            text::reminders::NO_REMINDERS
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
            no_calendar(),
            OWNER,
            &chat(),
            &Command::Reminder("каждый месяц 20".into()),
        )
        .await;

        let reply = respond(
            &service,
            no_calendar(),
            OWNER,
            &chat(),
            &Command::Reminder("remove the invoice reminder".into()),
        )
        .await;
        assert!(reply.contains("Deleted"), "{reply}");
        assert!(reply.contains("оформить invoice"), "{reply}");
        assert!(
            service.reminders().list(OWNER).await.unwrap().is_empty(),
            "it is gone"
        );
    }

    #[tokio::test]
    async fn a_checklist_is_created_and_ticked_off_line_by_line() {
        let service = service(
            ScriptedLlm::new()
                .calling(vec![ToolCall::new(
                    "call_0",
                    "create_reminder",
                    serde_json::json!({
                        "kind": "monthly", "time": "09:00", "days_of_month": [1, 2, 3, 4, 5, 6],
                        "text": "monthly payment",
                        "items": ["exchange money", "transfer to tbc", "write to the banker"]
                    })
                    .to_string(),
                )])
                .calling(vec![ToolCall::new(
                    "call_1",
                    "complete_items",
                    serde_json::json!({ "id": FIRST_ID, "items": ["exchange money"] }).to_string(),
                )]),
        );

        let created = respond(
            &service,
            no_calendar(),
            OWNER,
            &chat(),
            &Command::Reminder("pay every month, 1st to 6th".into()),
        )
        .await;
        assert!(created.contains("monthly payment"), "{created}");

        let ticked = respond(
            &service,
            no_calendar(),
            OWNER,
            &chat(),
            &Command::Reminder("exchanged the money".into()),
        )
        .await;
        assert!(ticked.contains("✅ exchange money"), "{ticked}");
        assert!(ticked.contains("⚪ transfer to tbc"), "{ticked}");
        assert!(ticked.contains("⚪ write to the banker"), "{ticked}");
    }

    #[tokio::test]
    async fn ticking_the_last_line_ends_the_period_by_itself() {
        let service = service(
            ScriptedLlm::new()
                .calling(vec![ToolCall::new(
                    "call_0",
                    "create_reminder",
                    serde_json::json!({
                        "kind": "monthly", "time": "09:00", "days_of_month": [1, 2, 3],
                        "text": "monthly payment",
                        "items": ["exchange money", "transfer to tbc"]
                    })
                    .to_string(),
                )])
                .calling(vec![ToolCall::new(
                    "call_1",
                    "complete_items",
                    serde_json::json!({
                        "id": FIRST_ID, "items": ["exchange money", "transfer to tbc"]
                    })
                    .to_string(),
                )]),
        );
        respond(
            &service,
            no_calendar(),
            OWNER,
            &chat(),
            &Command::Reminder("pay monthly".into()),
        )
        .await;

        let done = respond(
            &service,
            no_calendar(),
            OWNER,
            &chat(),
            &Command::Reminder("did both of them".into()),
        )
        .await;
        assert!(done.contains("All done"), "{done}");
        assert!(done.contains("quiet until"), "{done}");

        // Still there, just silent for the rest of this period.
        let stored = service.reminders().list(OWNER).await.unwrap();
        assert_eq!(stored.len(), 1);
        assert!(stored[0].acknowledged_through.is_some());
    }

    #[tokio::test]
    async fn ticking_a_line_on_a_reminder_with_no_checklist_says_so() {
        let service = service(create_then(
            "complete_items",
            serde_json::json!({ "id": FIRST_ID, "items": ["something"] }),
        ));
        respond(
            &service,
            no_calendar(),
            OWNER,
            &chat(),
            &Command::Reminder("каждый месяц 20".into()),
        )
        .await;

        let reply = respond(
            &service,
            no_calendar(),
            OWNER,
            &chat(),
            &Command::Reminder("done with it".into()),
        )
        .await;
        assert!(reply.contains("no checklist"), "{reply}");
    }

    #[tokio::test]
    async fn the_model_can_acknowledge_rather_than_delete() {
        let service = service(create_then(
            "acknowledge_reminder",
            serde_json::json!({ "id": FIRST_ID }),
        ));
        respond(
            &service,
            no_calendar(),
            OWNER,
            &chat(),
            &Command::Reminder("каждый месяц 20".into()),
        )
        .await;

        let reply = respond(
            &service,
            no_calendar(),
            OWNER,
            &chat(),
            &Command::Reminder("already sent the invoice".into()),
        )
        .await;
        assert!(reply.contains("Done"), "{reply}");
        assert!(reply.contains("quiet until"), "{reply}");
        assert_eq!(
            service.reminders().list(OWNER).await.unwrap().len(),
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
            no_calendar(),
            OWNER,
            &chat(),
            &Command::Reminder("каждый месяц 20".into()),
        )
        .await;

        let reply = respond(
            &service,
            no_calendar(),
            OWNER,
            &chat(),
            &Command::Reminder("make it the 20th to the 26th at 22:30".into()),
        )
        .await;
        assert!(reply.contains("Updated"), "{reply}");
        assert!(
            reply.contains("every month on the 20th to the 26th at 22:30"),
            "{reply}"
        );
        let stored = service.reminders().list(OWNER).await.unwrap();
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
            no_calendar(),
            OWNER,
            &chat(),
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
            no_calendar(),
            OWNER,
            &chat(),
            &Command::Reminder("каждый месяц 20".into()),
        )
        .await;

        let reply = respond(
            &service,
            no_calendar(),
            OTHER,
            &chat(),
            &Command::Reminder("remove it".into()),
        )
        .await;
        assert!(reply.contains("No reminder with id"), "{reply}");
        assert_eq!(
            service.reminders().list(OWNER).await.unwrap().len(),
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
            no_calendar(),
            OWNER,
            &chat(),
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
        let reply = respond(
            &service,
            no_calendar(),
            OWNER,
            &chat(),
            &Command::Reminder("do something".into()),
        )
        .await;
        // Falls through to prose rather than being treated as an action.
        assert!(!reply.contains("Deleted"), "{reply}");
    }
}

#[cfg(test)]
mod calendar_tests {
    use std::sync::Arc;

    use serana_domain::calendar::{CalendarEvent, EventId};
    use serana_domain::message::ToolCall;
    use serana_domain::reminder::TimeZoneName;
    use serana_services::{CalendarService, Chat, Planning};
    use serana_testkit::{
        FixedClock, InMemoryCalendar, InMemoryConversationRepository, ScriptedLlm,
    };

    use super::tests::{
        Calendar, NOW, OWNER, TRAVEL, chat, chat_config, service, service_with_llm,
    };
    use super::*;

    fn event(id: &str, summary: &str, starts_at: &str, ends_at: &str) -> CalendarEvent {
        CalendarEvent {
            id: EventId::new(id),
            summary: summary.into(),
            starts_at: starts_at.parse().unwrap(),
            ends_at: ends_at.parse().unwrap(),
            all_day: false,
            location: None,
            mine: true,
            recurring: false,
        }
    }

    fn calendar(llm: Arc<ScriptedLlm>, events: InMemoryCalendar) -> Calendar {
        Chat::new(
            Planning::new(CalendarService::new(
                events,
                FixedClock::at(NOW),
                TimeZoneName::new("Asia/Tbilisi"),
                TRAVEL,
            )),
            llm,
            InMemoryConversationRepository::new(),
            chat_config(),
        )
    }

    fn calling(name: &str, arguments: serde_json::Value) -> Arc<ScriptedLlm> {
        Arc::new(ScriptedLlm::new().calling(vec![ToolCall::new("c1", name, arguments.to_string())]))
    }

    #[tokio::test]
    async fn a_calendar_request_with_none_configured_says_so_rather_than_apologising() {
        // Nothing has gone wrong: the assistant simply has no calendar.
        let (service, llm) = service_with_llm(ScriptedLlm::new());
        let reply = respond(
            &service,
            None::<&Calendar>,
            OWNER,
            &chat(),
            &Command::Calendar("what's on?".into()),
        )
        .await;
        assert_eq!(reply, text::calendar::NOT_CONNECTED);
        assert_eq!(llm.call_count(), 0, "no tokens spent with nothing to ask");
    }

    #[tokio::test]
    async fn a_bare_calendar_command_asks_what_for_without_calling_the_model() {
        let llm = Arc::new(ScriptedLlm::new());
        let calendar = calendar(Arc::clone(&llm), InMemoryCalendar::new());
        let service = service(ScriptedLlm::new());
        for argument in ["", "   "] {
            let reply = respond(
                &service,
                Some(&calendar),
                OWNER,
                &chat(),
                &Command::Calendar(argument.into()),
            )
            .await;
            assert_eq!(reply, text::calendar::CALENDAR_NEEDS_TEXT);
        }
        assert_eq!(llm.call_count(), 0);
    }

    #[tokio::test]
    async fn booking_something_in_person_reserves_the_road_and_says_so() {
        let llm = calling(
            "create_event",
            serde_json::json!({
                "summary": "стоматолог",
                "date": "2026-03-11",
                "start_time": "14:00",
                "end_time": "15:00",
                "location": "Chavchavadze 1",
                "in_person": true,
            }),
        );
        let calendar = calendar(Arc::clone(&llm), InMemoryCalendar::new());
        let reply = respond(
            &service(ScriptedLlm::new()),
            Some(&calendar),
            OWNER,
            &chat(),
            &Command::Calendar("стоматолог завтра в 2".into()),
        )
        .await;

        assert!(reply.contains("стоматолог"), "their own words: {reply}");
        assert!(reply.contains("14:00–15:00"), "the appointment: {reply}");
        assert!(reply.contains("13:30–15:30"), "the block: {reply}");
        assert!(reply.contains("Chavchavadze 1"), "{reply}");
    }

    #[tokio::test]
    async fn cancelling_an_invitation_declines_it_and_says_it_is_still_there() {
        let mut theirs = event(
            "ev1",
            "design review",
            "2026-03-11T10:00:00Z",
            "2026-03-11T11:00:00Z",
        );
        theirs.mine = false;
        let events = InMemoryCalendar::new().with(theirs);
        let llm = calling("cancel_event", serde_json::json!({ "id": "ev1" }));
        let calendar = calendar(Arc::clone(&llm), events);

        let reply = respond(
            &service(ScriptedLlm::new()),
            Some(&calendar),
            OWNER,
            &chat(),
            &Command::Calendar("I am not going to the design review".into()),
        )
        .await;
        assert!(reply.contains("Declined"), "{reply}");
        assert!(reply.contains("stays on your calendar"), "{reply}");
    }

    #[tokio::test]
    async fn an_event_the_model_invented_is_reported_rather_than_acted_on() {
        let llm = calling("cancel_event", serde_json::json!({ "id": "made-up" }));
        let calendar = calendar(Arc::clone(&llm), InMemoryCalendar::new());
        let reply = respond(
            &service(ScriptedLlm::new()),
            Some(&calendar),
            OWNER,
            &chat(),
            &Command::Calendar("cancel the dentist".into()),
        )
        .await;
        assert!(reply.contains("made-up"), "{reply}");
        assert!(reply.contains("not on your calendar"), "{reply}");
    }

    #[tokio::test]
    async fn the_question_asked_before_a_delete_reaches_the_user_verbatim() {
        // Deleting is the one irreversible action here, and the model is told to ask first.
        // Its question is the whole reply: paraphrasing it would lose what is being deleted.
        let question = "Delete “design review” on 11 March for good? This cannot be undone.";
        let llm = Arc::new(ScriptedLlm::new().answering(question));
        let calendar = calendar(
            Arc::clone(&llm),
            InMemoryCalendar::new().with(event(
                "ev1",
                "design review",
                "2026-03-11T10:00:00Z",
                "2026-03-11T11:00:00Z",
            )),
        );
        let reply = respond(
            &service(ScriptedLlm::new()),
            Some(&calendar),
            OWNER,
            &chat(),
            &Command::Calendar("delete the design review".into()),
        )
        .await;
        assert_eq!(reply, question);
    }

    #[tokio::test]
    async fn a_calendar_that_is_down_is_not_reported_as_a_misunderstanding() {
        let events = InMemoryCalendar::broken(CalendarError::Unavailable("503".into()));
        let llm = Arc::new(ScriptedLlm::new());
        let calendar = calendar(Arc::clone(&llm), events);
        let reply = respond(
            &service(ScriptedLlm::new()),
            Some(&calendar),
            OWNER,
            &chat(),
            &Command::Calendar("what's on?".into()),
        )
        .await;
        assert!(reply.contains("not answering"), "{reply}");
        assert_eq!(llm.call_count(), 0, "the week is read before the model is");
    }

    #[tokio::test]
    async fn folding_up_with_nothing_said_anywhere_says_there_was_nothing_to_fold() {
        let llm = Arc::new(ScriptedLlm::new());
        let calendar = calendar(Arc::clone(&llm), InMemoryCalendar::new());
        let reply = respond(
            &service(ScriptedLlm::new()),
            Some(&calendar),
            OWNER,
            &chat(),
            &Command::Compact,
        )
        .await;
        assert_eq!(reply, text::NOTHING_TO_COMPACT);
    }

    #[tokio::test]
    async fn folding_up_covers_the_calendar_conversation_too() {
        // The person asked to fold "this" up, and has no way to know there are two.
        let llm = Arc::new(
            ScriptedLlm::new()
                .calling(vec![ToolCall::new(
                    "c1",
                    "list_events",
                    serde_json::json!({ "from": "2026-03-11", "to": "2026-03-11" }).to_string(),
                )])
                .answering("they asked what was on"),
        );
        let calendar = calendar(Arc::clone(&llm), InMemoryCalendar::new());
        let service = service(ScriptedLlm::new());

        respond(
            &service,
            Some(&calendar),
            OWNER,
            &chat(),
            &Command::Calendar("what's on tomorrow?".into()),
        )
        .await;
        let reply = respond(&service, Some(&calendar), OWNER, &chat(), &Command::Compact).await;

        assert_eq!(reply, text::COMPACTED);
        assert_eq!(llm.remaining(), 0, "the summary call happened");
    }
}
