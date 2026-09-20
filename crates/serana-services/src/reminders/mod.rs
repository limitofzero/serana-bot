//! Creating, listing and cancelling reminders.

mod parse;
mod scheduler;

use std::sync::Arc;

use serana_domain::llm::{CompletionRequest, LlmProvider};
use serana_domain::message::Message;
use serana_domain::reminder::{Reminder, ReminderId, ReminderRepository, TimeZoneName, UserId};
use serana_domain::tool::ToolSpec;
use serana_domain::{Clock, IdGenerator, LlmError, StorageError};

use parse::ParsedReminder;

pub use scheduler::{SchedulerService, TickReport};

/// The function the model is asked to call. Named for what it does to the user's request,
/// not for our internals, because the name is part of the prompt the model reads.
const PARSE_TOOL: &str = "schedule_reminder";

#[derive(Debug, thiserror::Error)]
pub enum ReminderError {
    /// The request could not be turned into a schedule. The message is written to be shown
    /// to the user, because that is where it ends up.
    #[error("could not understand the schedule: {0}")]
    Unparsable(String),

    /// The schedule is well-formed but has no occurrence ahead of now — a one-off in the
    /// past, or a monthly reminder for a day that does not exist.
    #[error("that schedule has no future occurrence")]
    NeverFires,

    #[error("no reminder with id {0}")]
    NotFound(ReminderId),

    #[error(transparent)]
    Llm(#[from] LlmError),

    #[error(transparent)]
    Storage(#[from] StorageError),
}

#[derive(Debug, Clone)]
pub struct ReminderConfig {
    /// Model used to read the request. A small one is enough; this is extraction, not
    /// reasoning.
    pub model: String,
    /// Zone every new reminder is created in.
    pub default_timezone: TimeZoneName,
}

/// Reminder lifecycle, independent of how the request arrived.
///
/// Telegram's `/reminder` command calls this, and so will a `create_reminder` tool once the
/// agent can set reminders conversationally. Neither knows anything the other does not.
pub struct ReminderService<R, L, C, I> {
    repository: R,
    llm: L,
    clock: C,
    ids: I,
    config: ReminderConfig,
}

impl<R, L, C, I> ReminderService<R, L, C, I>
where
    R: ReminderRepository,
    L: LlmProvider,
    C: Clock,
    I: IdGenerator,
{
    pub fn new(repository: R, llm: L, clock: C, ids: I, config: ReminderConfig) -> Self {
        Self {
            repository,
            llm,
            clock,
            ids,
            config,
        }
    }

    /// Read `request` as a schedule and store the reminder it describes.
    pub async fn create(&self, owner: UserId, request: &str) -> Result<Reminder, ReminderError> {
        if request.trim().is_empty() {
            return Err(ReminderError::Unparsable("the request is empty".into()));
        }

        let now = self.clock.now();
        let timezone = self.config.default_timezone.clone();
        let local = now.to_zoned(timezone.resolve()?);

        let response = self
            .llm
            .complete(
                CompletionRequest::new(
                    &self.config.model,
                    Arc::from(parsing_prompt(&local, &timezone).as_str()),
                    vec![Message::user(request)],
                )
                .with_tools(vec![ToolSpec::typed::<ParsedReminder>(
                    PARSE_TOOL,
                    "Record the schedule and text of the reminder the person asked for.",
                )])
                // Extraction, not creativity: the same request should yield the same
                // schedule every time.
                .with_temperature(0.0),
            )
            .await?;

        let call = response
            .tool_calls
            .into_iter()
            .find(|call| call.name == PARSE_TOOL)
            .ok_or_else(|| {
                // The model answered in prose instead of calling the function. Its text is
                // usually an explanation of why it could not, so it is worth surfacing.
                ReminderError::Unparsable(
                    response
                        .content
                        .as_deref()
                        .map(str::trim)
                        .filter(|text| !text.is_empty())
                        .unwrap_or("the request did not describe a schedule")
                        .to_owned(),
                )
            })?;

        let arguments = call.parse_arguments().map_err(|e| {
            ReminderError::Unparsable(format!("the schedule came back malformed: {e}"))
        })?;
        let parsed: ParsedReminder = serde_json::from_value(arguments)
            .map_err(|e| ReminderError::Unparsable(format!("the schedule is incomplete: {e}")))?;
        let (recurrence, text) = parsed.into_recurrence()?;

        let mut reminder = Reminder {
            id: ReminderId::new(self.ids.generate()),
            owner,
            text,
            recurrence,
            timezone,
            created_at: now,
            next_fire_at: None,
            last_fired_at: None,
        };

        // Refuse before storing rather than after: a reminder that can never fire is
        // clutter the user would have to find and delete.
        if reminder.reschedule(now)?.is_none() {
            return Err(ReminderError::NeverFires);
        }

        self.repository.put(&reminder).await?;
        Ok(reminder)
    }

    /// Everything `owner` has, soonest first.
    pub async fn list(&self, owner: UserId) -> Result<Vec<Reminder>, ReminderError> {
        Ok(self.repository.list_for_owner(owner).await?)
    }

    /// Cancel one of `owner`'s reminders.
    ///
    /// A reminder belonging to somebody else reports [`ReminderError::NotFound`] rather
    /// than a permission error, so ids cannot be probed for existence.
    pub async fn delete(&self, owner: UserId, id: &ReminderId) -> Result<Reminder, ReminderError> {
        let reminder = self
            .repository
            .get(id)
            .await?
            .filter(|reminder| reminder.owner == owner)
            .ok_or_else(|| ReminderError::NotFound(id.clone()))?;

        self.repository.delete(id).await?;
        Ok(reminder)
    }
}

/// The system prompt for the extraction call.
///
/// It carries the current local time because "tomorrow" and "next Friday" are meaningless
/// without it, and the model has no clock of its own.
fn parsing_prompt(local: &jiff::Zoned, timezone: &TimeZoneName) -> String {
    format!(
        "You convert a person's reminder request into a schedule.\n\
         \n\
         Their current local date and time is {date} {time} ({weekday}), time zone {zone}.\n\
         Resolve relative expressions such as \"tomorrow\", \"next Friday\" or \"in an hour\" \
         against that moment.\n\
         \n\
         Call the `{tool}` function exactly once. Never answer in prose when the request \
         describes a schedule.\n\
         If the request genuinely contains no schedule, reply with one short sentence \
         saying what is missing.\n\
         \n\
         Keep the reminder text in the language the person wrote it in, and keep it in their \
         own words. Strip the scheduling part out of it: \"every month on the 20th, tell me \
         to issue an invoice\" has the text \"issue an invoice\".\n\
         If no time of day is given, use 09:00.",
        date = local.date(),
        time = local.time().strftime("%H:%M"),
        weekday = local.strftime("%A"),
        zone = timezone,
        tool = PARSE_TOOL,
    )
}

#[cfg(test)]
mod tests {
    use serana_domain::message::ToolCall;
    use serana_domain::reminder::{Recurrence, Weekday};
    use serana_testkit::{FixedClock, InMemoryReminderRepository, ScriptedLlm, SequentialIds};

    use super::*;

    const OWNER: UserId = UserId::new(42);
    const OTHER: UserId = UserId::new(7);

    type Service =
        ReminderService<InMemoryReminderRepository, ScriptedLlm, FixedClock, SequentialIds>;

    fn config() -> ReminderConfig {
        ReminderConfig {
            model: "gpt-5-mini".into(),
            default_timezone: TimeZoneName::new("Asia/Tbilisi"),
        }
    }

    /// A service whose model is scripted, whose clock is fixed, and whose ids count up.
    fn service(llm: ScriptedLlm) -> Service {
        ReminderService::new(
            InMemoryReminderRepository::new(),
            llm,
            // 10:00 local in Tbilisi.
            FixedClock::at("2026-03-10T06:00:00Z"),
            SequentialIds::default(),
            config(),
        )
    }

    /// A model that calls the extraction function with `arguments`.
    fn extracting(arguments: serde_json::Value) -> ScriptedLlm {
        ScriptedLlm::new().calling(vec![ToolCall::new(
            "call_1",
            PARSE_TOOL,
            arguments.to_string(),
        )])
    }

    fn monthly_invoice() -> serde_json::Value {
        serde_json::json!({
            "kind": "monthly",
            "time": "10:00",
            "day_of_month": 20,
            "text": "оформить invoice"
        })
    }

    #[tokio::test]
    async fn the_request_from_the_brief_becomes_a_stored_monthly_reminder() {
        let service = service(extracting(monthly_invoice()));
        let reminder = service
            .create(
                OWNER,
                "каждый месяц 20 число - писать мне что надо оформить invoice",
            )
            .await
            .unwrap();

        assert_eq!(
            reminder.recurrence,
            Recurrence::Monthly {
                day: 20,
                at: jiff::civil::time(10, 0, 0, 0)
            }
        );
        assert_eq!(reminder.text, "оформить invoice");
        assert_eq!(reminder.owner, OWNER);
        assert_eq!(reminder.id.as_str(), "r1");
        // 10:00 Tbilisi on the 20th is 06:00 UTC.
        assert_eq!(
            reminder.next_fire_at,
            Some("2026-03-20T06:00:00Z".parse().unwrap())
        );

        let stored = service.repository.get(&reminder.id).await.unwrap();
        assert_eq!(stored, Some(reminder));
    }

    #[tokio::test]
    async fn every_recurrence_kind_can_be_created() {
        for (arguments, expected) in [
            (
                serde_json::json!({"kind": "daily", "time": "08:30", "text": "зарядка"}),
                Recurrence::Daily {
                    at: jiff::civil::time(8, 30, 0, 0),
                },
            ),
            (
                serde_json::json!({"kind": "weekly", "time": "18:00", "weekday": "friday",
                                   "text": "отчёт"}),
                Recurrence::Weekly {
                    weekday: Weekday::Friday,
                    at: jiff::civil::time(18, 0, 0, 0),
                },
            ),
            (
                serde_json::json!({"kind": "once", "time": "12:00", "date": "2026-04-01",
                                   "text": "позвонить"}),
                Recurrence::Once {
                    at: jiff::civil::date(2026, 4, 1).at(12, 0, 0, 0),
                },
            ),
        ] {
            let service = service(extracting(arguments));
            let reminder = service.create(OWNER, "что-нибудь").await.unwrap();
            assert_eq!(reminder.recurrence, expected);
            assert!(reminder.is_active());
        }
    }

    #[tokio::test]
    async fn the_extraction_call_carries_the_current_local_time_and_offers_the_function() {
        // Without the local time in the prompt, "tomorrow" cannot be resolved at all.
        let service = service(extracting(monthly_invoice()));
        service
            .create(OWNER, "каждый месяц 20 число")
            .await
            .unwrap();

        let request = &service.llm.requests()[0];
        assert!(
            request.system_prompt.contains("2026-03-10"),
            "{}",
            request.system_prompt
        );
        assert!(
            request.system_prompt.contains("10:00"),
            "{}",
            request.system_prompt
        );
        assert!(
            request.system_prompt.contains("Tuesday"),
            "{}",
            request.system_prompt
        );
        assert!(
            request.system_prompt.contains("Asia/Tbilisi"),
            "{}",
            request.system_prompt
        );
        assert_eq!(request.tools.len(), 1);
        assert_eq!(request.tools[0].name, PARSE_TOOL);
        assert_eq!(
            request.temperature,
            Some(0.0),
            "extraction must be deterministic"
        );
        assert_eq!(request.model, "gpt-5-mini");
    }

    #[tokio::test]
    async fn an_empty_request_is_refused_without_asking_the_model() {
        let service = service(ScriptedLlm::new());
        for request in ["", "   ", "\n"] {
            assert!(matches!(
                service.create(OWNER, request).await,
                Err(ReminderError::Unparsable(_))
            ));
        }
        assert_eq!(
            service.llm.call_count(),
            0,
            "no tokens spent on an empty request"
        );
    }

    #[tokio::test]
    async fn prose_instead_of_a_function_call_surfaces_the_models_own_explanation() {
        let service = service(ScriptedLlm::new().answering("I need to know what time of day."));
        let err = service
            .create(OWNER, "напомни про invoice")
            .await
            .unwrap_err();
        assert!(
            matches!(&err, ReminderError::Unparsable(message)
                     if message == "I need to know what time of day."),
            "{err:?}"
        );
        assert!(service.repository.is_empty());
    }

    #[tokio::test]
    async fn an_empty_prose_answer_still_produces_a_usable_message() {
        let service = service(ScriptedLlm::new().answering("   "));
        let err = service.create(OWNER, "что-то").await.unwrap_err();
        assert!(
            err.to_string().contains("did not describe a schedule"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn malformed_function_arguments_are_reported_not_panicked_on() {
        let service = service(ScriptedLlm::new().calling(vec![ToolCall::new(
            "c1",
            PARSE_TOOL,
            r#"{"kind":"monthly"#,
        )]));
        let err = service.create(OWNER, "что-то").await.unwrap_err();
        assert!(matches!(err, ReminderError::Unparsable(_)), "{err:?}");
        assert!(service.repository.is_empty());
    }

    #[tokio::test]
    async fn arguments_missing_a_required_field_are_reported() {
        let service = service(extracting(serde_json::json!({"kind": "daily"})));
        let err = service.create(OWNER, "что-то").await.unwrap_err();
        assert!(matches!(err, ReminderError::Unparsable(_)), "{err:?}");
    }

    #[tokio::test]
    async fn a_schedule_with_no_future_occurrence_is_refused_and_nothing_is_stored() {
        // A one-off in the past. Storing it would leave the user something to find and
        // delete that was never going to do anything.
        let service = service(extracting(serde_json::json!({
            "kind": "once", "time": "10:00", "date": "2020-01-01", "text": "прошлое"
        })));
        assert!(matches!(
            service.create(OWNER, "что-то").await,
            Err(ReminderError::NeverFires)
        ));
        assert!(service.repository.is_empty());
    }

    #[tokio::test]
    async fn a_provider_failure_propagates_rather_than_being_reported_as_unparsable() {
        // The user should be told the model was unreachable, not that their phrasing was bad.
        let service = service(ScriptedLlm::new().failing(LlmError::RateLimited {
            retry_after_secs: Some(30),
        }));
        let err = service.create(OWNER, "что-то").await.unwrap_err();
        assert!(
            matches!(err, ReminderError::Llm(LlmError::RateLimited { .. })),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn a_listing_is_scoped_to_its_owner() {
        let service = service(extracting(monthly_invoice()).calling(vec![ToolCall::new(
            "c2",
            PARSE_TOOL,
            monthly_invoice().to_string(),
        )]));
        service.create(OWNER, "мне").await.unwrap();
        service.create(OTHER, "им").await.unwrap();

        let mine = service.list(OWNER).await.unwrap();
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].owner, OWNER);
        assert_eq!(service.list(UserId::new(999)).await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn deleting_your_own_reminder_removes_it_and_returns_it() {
        let service = service(extracting(monthly_invoice()));
        let created = service.create(OWNER, "что-то").await.unwrap();

        let deleted = service.delete(OWNER, &created.id).await.unwrap();
        assert_eq!(deleted.id, created.id);
        assert!(service.repository.is_empty());
    }

    #[tokio::test]
    async fn deleting_someone_elses_reminder_reports_not_found_and_leaves_it_alone() {
        // Not a permission error: that would confirm the id exists, and ids are guessable.
        let service = service(extracting(monthly_invoice()));
        let created = service.create(OWNER, "что-то").await.unwrap();

        let err = service.delete(OTHER, &created.id).await.unwrap_err();
        assert!(matches!(err, ReminderError::NotFound(_)), "{err:?}");
        assert!(service.repository.get(&created.id).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn deleting_an_unknown_id_reports_not_found() {
        let service = service(ScriptedLlm::new());
        let err = service
            .delete(OWNER, &ReminderId::new("nope"))
            .await
            .unwrap_err();
        assert!(matches!(err, ReminderError::NotFound(_)), "{err:?}");
    }
}
