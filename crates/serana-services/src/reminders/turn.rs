//! One turn of conversation about reminders.
//!
//! This is where the model lives: the conversation is loaded, the request is put to the
//! provider with a toolset, whatever it chose is dispatched to [`ReminderService`], and the
//! exchange is written back. It knows how a request arrives and what to do with the answer;
//! it knows nothing about what a reminder *is*.

use std::sync::Arc;

use serana_domain::conversation::{Conversation, ConversationId};
use serana_domain::conversation_store::ConversationRepository;
use serana_domain::llm::{CompletionRequest, CompletionResponse, LlmProvider};
use serana_domain::message::Message;
use serana_domain::reminder::{ReminderId, ReminderRepository, TimeZoneName, UserId};
use serana_domain::{Clock, IdGenerator, StorageError};

use super::config::ReminderConfig;
use super::error::ReminderError;
use super::outcome::{ReminderOutcome, summarise};
use super::parse::ParsedReminder;
use super::service::ReminderService;
use super::{prompt, tools};

/// A conversation about reminders.
pub struct ChatService<R, L, C, I, V> {
    reminders: ReminderService<R, C, I>,
    llm: L,
    conversations: V,
    config: ReminderConfig,
}

impl<R, L, C, I, V> ChatService<R, L, C, I, V>
where
    R: ReminderRepository,
    L: LlmProvider,
    C: Clock,
    I: IdGenerator,
    V: ConversationRepository,
{
    pub fn new(
        reminders: ReminderService<R, C, I>,
        llm: L,
        conversations: V,
        config: ReminderConfig,
    ) -> Self {
        Self {
            reminders,
            llm,
            conversations,
            config,
        }
    }

    /// The reminders underneath. Listing and the clock come from here; the frontend needs
    /// both and neither is a conversation concern.
    pub fn reminders(&self) -> &ReminderService<R, C, I> {
        &self.reminders
    }

    /// Give up the reminders, so another conversation can be had over the same storage.
    ///
    /// A turn is one model call, so a test covering two turns needs two chat services over
    /// one set of reminders. Consuming rather than cloning keeps that explicit.
    pub fn into_reminders(self) -> ReminderService<R, C, I> {
        self.reminders
    }

    pub async fn handle(
        &self,
        owner: UserId,
        conversation: &ConversationId,
        request: &str,
    ) -> Result<ReminderOutcome, ReminderError> {
        if request.trim().is_empty() {
            return Err(ReminderError::Unparsable("the request is empty".into()));
        }

        let now = self.reminders.now();
        let timezone = self.config.default_timezone.clone();
        let local = now.to_zoned(timezone.resolve()?);
        let existing = self.reminders.list_all(owner).await?;

        let mut history = self
            .conversations
            .load(conversation)
            .await?
            .unwrap_or_else(|| Conversation::new(conversation.clone(), prompt::INSTRUCTIONS, now));

        history
            .push_user(format!(
                "{}\n{request}",
                prompt::context(&local, &timezone, &existing, now)
            ))
            .map_err(|e| ReminderError::Storage(StorageError::Corrupt(e.to_string())))?;

        let response = self.ask(&history).await?;

        // A name we do not offer is a hallucination, not an action: fall through to the
        // model's own prose rather than dispatching something we cannot check.
        let chosen = response
            .tool_calls
            .iter()
            .find(|call| tools::is_known(&call.name))
            .cloned();

        let outcome = match chosen {
            None => ReminderOutcome::Said(
                response
                    .content
                    .as_deref()
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .unwrap_or("I could not work out what you wanted there.")
                    .to_owned(),
            ),
            Some(call) => {
                let arguments = call.parse_arguments().map_err(|e| {
                    ReminderError::Unparsable(format!("the answer came back malformed: {e}"))
                })?;
                self.dispatch(owner, now, &timezone, &call.name, arguments)
                    .await?
            }
        };

        self.record(&mut history, &response, &outcome)?;

        // Compaction is the one sanctioned cache break, so it happens as late as possible:
        // only once the provider says the history it is re-reading has grown past the
        // threshold. `prompt_tokens` is the provider's own count, so there is no tokenizer
        // to take on and no estimate to be wrong about.
        if response.usage.prompt_tokens > self.config.compact_above_tokens {
            self.compact(&mut history).await?;
        }

        self.conversations.save(&history).await?;
        Ok(outcome)
    }

    /// Ask the model, with the whole conversation in front of it.
    async fn ask(&self, history: &Conversation) -> Result<CompletionResponse, ReminderError> {
        let mut call = CompletionRequest::new(
            &self.config.model,
            Arc::from(history.system_prompt()),
            history.messages().to_vec(),
        )
        .with_tools(tools::specs());
        if let Some(temperature) = self.config.temperature {
            call = call.with_temperature(temperature);
        }
        Ok(self.llm.complete(call).await?)
    }

    /// Append what the model said, and what came of it, to the history.
    ///
    /// A tool call must be followed by exactly one result, on every path — leave one open
    /// and the next turn starts `tool -> user`, which providers reject.
    fn record(
        &self,
        history: &mut Conversation,
        response: &CompletionResponse,
        outcome: &ReminderOutcome,
    ) -> Result<(), ReminderError> {
        let corrupt = |e: serana_domain::AlternationError| {
            ReminderError::Storage(StorageError::Corrupt(e.to_string()))
        };

        match outcome {
            ReminderOutcome::Said(words) => history.push_assistant(words).map_err(corrupt),
            _ => {
                let calls = response.tool_calls.clone();
                let Some(first) = calls.first().cloned() else {
                    return Ok(());
                };
                history.push_tool_calls(calls).map_err(corrupt)?;
                history
                    .push_tool_result(&first.id, summarise(outcome))
                    .map_err(corrupt)?;
                // Anything the model asked for beyond the first call was never run; closing
                // them keeps the alternation legal and tells the model why.
                history.close_open_tool_calls("not run: one action per turn");
                Ok(())
            }
        }
    }

    /// Summarise a stored conversation in place, at the user's request.
    ///
    /// Returns whether there was anything to summarise.
    pub async fn compact_conversation(
        &self,
        conversation: &ConversationId,
    ) -> Result<bool, ReminderError> {
        let Some(mut history) = self.conversations.load(conversation).await? else {
            return Ok(false);
        };
        if history.is_empty() {
            return Ok(false);
        }
        self.compact(&mut history).await?;
        self.conversations.save(&history).await?;
        Ok(true)
    }

    /// Replace the history with a summary of itself, keeping the system prompt.
    ///
    /// Triggered by [`ReminderConfig::compact_above_tokens`], or by the user asking. The
    /// summary arrives as a user message with an assistant acknowledgement after it, so the
    /// conversation is left able to accept the next user message.
    pub async fn compact(&self, history: &mut Conversation) -> Result<(), ReminderError> {
        if history.is_empty() {
            return Ok(());
        }

        let transcript = history
            .messages()
            .iter()
            .map(|message| match message {
                Message::User { content } => format!("them: {content}"),
                Message::Assistant {
                    content,
                    tool_calls,
                } => match content {
                    Some(text) => format!("you: {text}"),
                    None => format!(
                        "you: (called {})",
                        tool_calls
                            .iter()
                            .map(|c| c.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                },
                Message::Tool { content, .. } => format!("result: {content}"),
            })
            .collect::<Vec<_>>()
            .join("\n");

        let summary = self
            .llm
            .complete(CompletionRequest::new(
                self.config.summary_model(),
                Arc::from(prompt::SUMMARISE),
                vec![Message::user(transcript)],
            ))
            .await?
            .content
            .unwrap_or_default();

        let corrupt = |e: serana_domain::AlternationError| {
            ReminderError::Storage(StorageError::Corrupt(e.to_string()))
        };

        // Same id, same system prompt, same creation time: it is the same conversation,
        // with its middle replaced.
        let mut compacted = Conversation::new(
            history.id().clone(),
            history.system_prompt(),
            history.created_at(),
        );
        compacted
            .push_user(format!(
                "Notes on our earlier conversation:\n{}",
                summary.trim()
            ))
            .map_err(corrupt)?;
        compacted.push_assistant("Noted.").map_err(corrupt)?;
        *history = compacted;
        Ok(())
    }

    /// Run the action the model chose.
    async fn dispatch(
        &self,
        owner: UserId,
        now: jiff::Timestamp,
        timezone: &TimeZoneName,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<ReminderOutcome, ReminderError> {
        let incomplete = |e| ReminderError::Unparsable(format!("the answer was incomplete: {e}"));

        match name {
            tools::CREATE => {
                let parsed: ParsedReminder =
                    serde_json::from_value(arguments).map_err(incomplete)?;
                let (recurrence, text, items) = parsed.into_recurrence()?;
                Ok(ReminderOutcome::Created {
                    reminder: self
                        .reminders
                        .create(owner, now, timezone.clone(), recurrence, text, items)
                        .await?,
                    at: now,
                })
            }
            tools::UPDATE => {
                let args: tools::UpdateArgs =
                    serde_json::from_value(arguments).map_err(incomplete)?;
                let (recurrence, text, items) = args.schedule.into_recurrence()?;
                Ok(ReminderOutcome::Updated {
                    reminder: self
                        .reminders
                        .update(
                            owner,
                            &ReminderId::new(args.id.trim()),
                            recurrence,
                            text,
                            items,
                            now,
                        )
                        .await?,
                    at: now,
                })
            }
            tools::DELETE => {
                let args: tools::TargetArgs =
                    serde_json::from_value(arguments).map_err(incomplete)?;
                Ok(ReminderOutcome::Deleted(
                    self.reminders
                        .delete(owner, &ReminderId::new(args.id.trim()))
                        .await?,
                ))
            }
            tools::COMPLETE => {
                let args: tools::CompleteArgs =
                    serde_json::from_value(arguments).map_err(incomplete)?;
                let (reminder, ticked) = self
                    .reminders
                    .complete_items(owner, &ReminderId::new(args.id.trim()), &args.items, now)
                    .await?;
                Ok(ReminderOutcome::Completed {
                    reminder,
                    ticked,
                    at: now,
                })
            }
            tools::ACKNOWLEDGE => {
                let args: tools::TargetArgs =
                    serde_json::from_value(arguments).map_err(incomplete)?;
                Ok(ReminderOutcome::Acknowledged(
                    self.reminders
                        .acknowledge(owner, &ReminderId::new(args.id.trim()))
                        .await?,
                ))
            }
            // `is_known` gates the caller, so reaching here means the two lists disagree.
            other => Err(ReminderError::Unparsable(format!(
                "I do not know how to {other}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use serana_domain::message::{Message, ToolCall, Usage};
    use serana_domain::reminder::{MonthDays, Recurrence, Reminder, TodoItem, WeekDays, Weekday};
    use serana_domain::{CompletionResponse, LlmError};
    use serana_testkit::{
        FixedClock, InMemoryConversationRepository, InMemoryReminderRepository, ScriptedLlm,
        SequentialIds,
    };

    use super::super::config::DEFAULT_COMPACT_ABOVE_TOKENS;
    use super::*;

    const OWNER: UserId = UserId::new(42);
    const OTHER: UserId = UserId::new(7);

    type Service = ChatService<
        InMemoryReminderRepository,
        ScriptedLlm,
        FixedClock,
        SequentialIds,
        InMemoryConversationRepository,
    >;

    fn config() -> ReminderConfig {
        ReminderConfig {
            model: "gpt-5-mini".into(),
            default_timezone: TimeZoneName::new("Asia/Tbilisi"),
            temperature: None,
            summary_model: None,
            compact_above_tokens: DEFAULT_COMPACT_ABOVE_TOKENS,
        }
    }

    /// A service whose model is scripted, whose clock is fixed, and whose ids count up.
    fn service(llm: ScriptedLlm) -> Service {
        ChatService::new(
            ReminderService::new(
                InMemoryReminderRepository::new(),
                // 10:00 local in Tbilisi.
                FixedClock::at("2026-03-10T06:00:00Z"),
                SequentialIds::default(),
            ),
            llm,
            InMemoryConversationRepository::new(),
            config(),
        )
    }

    /// A model that calls the extraction function with `arguments`.
    fn extracting(arguments: serde_json::Value) -> ScriptedLlm {
        ScriptedLlm::new().calling(vec![ToolCall::new(
            "call_1",
            tools::CREATE,
            arguments.to_string(),
        )])
    }

    /// The conversation every test talks in, unless it is testing conversations.
    fn chat() -> ConversationId {
        ConversationId::new("test")
    }

    /// Run a turn and expect it to have created a reminder.
    async fn create(
        service: &Service,
        owner: UserId,
        request: &str,
    ) -> Result<Reminder, ReminderError> {
        match service.handle(owner, &chat(), request).await? {
            ReminderOutcome::Created { reminder, .. } => Ok(reminder),
            other => panic!("expected a creation, got {other:?}"),
        }
    }

    /// A model that calls `tool` with `arguments`.
    fn calling(tool: &str, arguments: serde_json::Value) -> ScriptedLlm {
        ScriptedLlm::new().calling(vec![ToolCall::new("call_1", tool, arguments.to_string())])
    }

    fn monthly_invoice() -> serde_json::Value {
        serde_json::json!({
            "kind": "monthly",
            "time": "10:00",
            "days_of_month": [20],
            "text": "оформить invoice"
        })
    }

    #[tokio::test]
    async fn the_request_from_the_brief_becomes_a_stored_monthly_reminder() {
        let service = service(extracting(monthly_invoice()));
        let reminder = create(
            &service,
            OWNER,
            "каждый месяц 20 число - писать мне что надо оформить invoice",
        )
        .await
        .unwrap();

        assert_eq!(
            reminder.recurrence,
            Recurrence::Monthly {
                days: MonthDays::new([20]).unwrap(),
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

        let stored = service
            .reminders()
            .repository
            .get(&reminder.id)
            .await
            .unwrap();
        assert_eq!(stored, Some(reminder));
    }

    #[tokio::test]
    async fn no_temperature_is_sent_unless_one_is_configured() {
        // Reasoning models reject every temperature but their own default, failing the
        // whole request with a 400 rather than clamping. Sending 0.0 for determinism made
        // every reminder impossible to create on gpt-5.
        let service = service(extracting(monthly_invoice()));
        create(&service, OWNER, "every month on the 20th")
            .await
            .unwrap();
        assert_eq!(service.llm.requests()[0].temperature, None);
    }

    #[tokio::test]
    async fn a_configured_temperature_is_sent_through() {
        let mut service = service(extracting(monthly_invoice()));
        service.config.temperature = Some(0.0);
        create(&service, OWNER, "every month on the 20th")
            .await
            .unwrap();
        assert_eq!(service.llm.requests()[0].temperature, Some(0.0));
    }

    #[tokio::test]
    async fn a_question_and_its_answer_are_one_conversation() {
        // The whole point of persistence: "tomorrow at 9" means nothing on its own.
        let service = service(
            ScriptedLlm::new()
                .answering("What time should I remind you?")
                .calling(vec![ToolCall::new(
                    "call_1",
                    tools::CREATE,
                    serde_json::json!({
                        "kind": "once", "time": "09:00", "date": "2026-03-11",
                        "text": "call the bank"
                    })
                    .to_string(),
                )]),
        );

        let asked = service
            .handle(OWNER, &chat(), "remind me to call the bank")
            .await
            .unwrap();
        assert_eq!(
            asked,
            ReminderOutcome::Said("What time should I remind you?".into())
        );

        let answered = service
            .handle(OWNER, &chat(), "tomorrow at 9")
            .await
            .unwrap();
        assert!(
            matches!(answered, ReminderOutcome::Created { .. }),
            "{answered:?}"
        );

        // The second call carried the first exchange, which is why the answer resolved.
        let second = &service.llm.requests()[1];
        assert_eq!(second.messages.len(), 3, "user, assistant, user");
        assert!(
            matches!(&second.messages[1], Message::Assistant { content: Some(c), .. }
                     if c == "What time should I remind you?")
        );
    }

    #[tokio::test]
    async fn separate_conversations_do_not_see_each_other() {
        let service = service(
            ScriptedLlm::new()
                .answering("What time?")
                .answering("Still, what time?"),
        );
        service.handle(OWNER, &chat(), "remind me").await.unwrap();
        service
            .handle(OWNER, &ConversationId::new("elsewhere"), "remind me")
            .await
            .unwrap();

        let second = &service.llm.requests()[1];
        assert_eq!(second.messages.len(), 1, "a fresh conversation: {second:?}");
    }

    #[tokio::test]
    async fn a_tool_call_leaves_a_result_behind_so_the_next_turn_is_legal() {
        // Every tool call needs exactly one result, or the next turn opens `tool -> user`
        // and the provider rejects it (docs/reference-notes.md §3).
        let service = service(extracting(monthly_invoice()));
        create(&service, OWNER, "каждый месяц 20").await.unwrap();

        let stored = service.conversations.load(&chat()).await.unwrap().unwrap();
        assert!(stored.pending_tool_calls().is_empty(), "{stored:?}");
        assert!(matches!(
            stored.messages().last(),
            Some(Message::Tool { .. })
        ));
        // And it is short: this rides along on every later turn.
        let Some(Message::Tool { content, .. }) = stored.messages().last() else {
            unreachable!()
        };
        assert!(
            content.len() < 80,
            "tool results are re-sent forever: {content}"
        );
    }

    #[tokio::test]
    async fn a_turn_that_never_reached_the_model_leaves_the_history_alone() {
        let service = service(
            ScriptedLlm::new()
                .answering("hello")
                .failing(LlmError::Transport("the provider is down".into())),
        );
        service.handle(OWNER, &chat(), "first").await.unwrap();
        assert!(service.handle(OWNER, &chat(), "second").await.is_err());

        let stored = service.conversations.load(&chat()).await.unwrap().unwrap();
        assert_eq!(stored.len(), 2, "no orphaned user message: {stored:?}");
    }

    #[tokio::test]
    async fn compacting_folds_the_history_into_a_summary_and_keeps_going() {
        let service = service(
            ScriptedLlm::new()
                .answering("What time?")
                .answering("Notes: they want a bank reminder, time still unknown."),
        );
        service
            .handle(OWNER, &chat(), "remind me to call the bank")
            .await
            .unwrap();

        assert!(service.compact_conversation(&chat()).await.unwrap());
        let stored = service.conversations.load(&chat()).await.unwrap().unwrap();

        assert_eq!(stored.len(), 2, "a summary and an acknowledgement");
        assert!(
            format!("{:?}", stored.messages()).contains("time still unknown"),
            "{stored:?}"
        );
        // Same conversation, so the cache-stable half is untouched.
        assert_eq!(stored.system_prompt(), prompt::INSTRUCTIONS);
        assert_eq!(stored.id(), &chat());
        // And it can still take the next message.
        assert_eq!(stored.tail(), serana_domain::Tail::Assistant);
    }

    #[tokio::test]
    async fn compacting_an_untouched_conversation_says_there_was_nothing_to_do() {
        let service = service(ScriptedLlm::new());
        assert!(!service.compact_conversation(&chat()).await.unwrap());
    }

    #[tokio::test]
    async fn the_history_is_folded_up_once_the_provider_says_it_has_grown() {
        let mut service = service(
            ScriptedLlm::new()
                .responding(CompletionResponse {
                    content: Some("one".into()),
                    tool_calls: Vec::new(),
                    finish_reason: serana_domain::FinishReason::Stop,
                    usage: Usage {
                        prompt_tokens: 5_000,
                        ..Usage::default()
                    },
                })
                .answering("summary of everything"),
        );
        // Below the reported count, so the turn trips it.
        service.config.compact_above_tokens = 1_000;

        service.handle(OWNER, &chat(), "first").await.unwrap();
        let stored = service.conversations.load(&chat()).await.unwrap().unwrap();
        assert_eq!(stored.len(), 2, "folded up straight away: {stored:?}");
        assert!(format!("{:?}", stored.messages()).contains("summary of everything"));
    }

    #[tokio::test]
    async fn updating_keeps_the_reminders_identity_and_history() {
        let service = service(extracting(monthly_invoice()));
        let created = create(&service, OWNER, "каждый месяц 20").await.unwrap();

        // A second turn, against a model that changes it to a week-long range at 22:30.
        let service = Service::new(
            service.into_reminders(),
            calling(
                tools::UPDATE,
                serde_json::json!({
                    "id": created.id.as_str(), "kind": "monthly", "time": "22:30",
                    "days_of_month": [20, 21, 22, 23, 24, 25, 26],
                    "text": "оформить invoice"
                }),
            ),
            InMemoryConversationRepository::new(),
            config(),
        );
        let outcome = service
            .handle(OWNER, &chat(), "make it the 20th to the 26th")
            .await
            .unwrap();
        let ReminderOutcome::Updated {
            reminder: updated, ..
        } = outcome
        else {
            panic!("expected an update, got {outcome:?}");
        };

        assert_eq!(updated.id, created.id, "same reminder, not a new one");
        assert_eq!(updated.created_at, created.created_at, "history is kept");
        assert_eq!(
            updated.recurrence,
            Recurrence::Monthly {
                days: MonthDays::new([20, 21, 22, 23, 24, 25, 26]).unwrap(),
                at: jiff::civil::time(22, 30, 0, 0)
            }
        );
        assert_eq!(
            service.reminders().list(OWNER).await.unwrap().len(),
            1,
            "not duplicated"
        );
    }

    #[tokio::test]
    async fn a_spent_one_off_can_still_be_edited() {
        // Its moment has passed, so rescheduling finds nothing — but refusing the edit
        // would leave it frozen, editable only by deletion.
        let service = service(extracting(serde_json::json!({
            "kind": "once", "time": "09:00", "date": "2026-03-15", "text": "call the bank"
        })));
        let created = create(&service, OWNER, "call the bank on the 15th")
            .await
            .unwrap();
        assert!(created.is_active());

        // Deliver it. A one-off has nothing after that, so it goes spent — exactly the
        // state a reminder is in the morning after it fired.
        let later = "2026-03-20T06:00:00Z".parse::<jiff::Timestamp>().unwrap();
        let mut spent = created.clone();
        spent.mark_fired(later).unwrap();
        assert!(!spent.is_active(), "nothing left to fire");
        service.reminders().repository.put(&spent).await.unwrap();

        let updated = service
            .reminders()
            .update(
                OWNER,
                &created.id,
                created.recurrence.clone(),
                "call the bank".into(),
                vec![TodoItem::new("find the number"), TodoItem::new("call")],
                later,
            )
            .await
            .unwrap();

        assert_eq!(updated.items.len(), 2, "the checklist was added");
        assert!(!updated.is_active(), "still spent, which is honest");
    }

    #[tokio::test]
    async fn an_edit_that_would_kill_a_live_reminder_is_still_refused() {
        let service = service(extracting(monthly_invoice()));
        let created = create(&service, OWNER, "каждый месяц 20").await.unwrap();
        assert!(created.is_active());

        let now = FixedClock::at("2026-03-10T06:00:00Z").now();
        let result = service
            .reminders()
            .update(
                OWNER,
                &created.id,
                Recurrence::Once {
                    at: jiff::civil::date(2020, 1, 1).at(9, 0, 0, 0),
                },
                "оформить invoice".into(),
                Vec::new(),
                now,
            )
            .await;

        assert!(
            matches!(result, Err(ReminderError::NeverFires)),
            "{result:?}"
        );
        let stored = service
            .reminders()
            .repository
            .get(&created.id)
            .await
            .unwrap()
            .unwrap();
        assert!(stored.is_active(), "left exactly as it was");
    }

    #[tokio::test]
    async fn a_plain_reminder_can_be_turned_into_a_checklist() {
        let service = service(extracting(monthly_invoice()));
        let created = create(&service, OWNER, "каждый месяц 20").await.unwrap();
        assert!(created.items.is_empty(), "starts as a plain reminder");

        let service = Service::new(
            service.into_reminders(),
            calling(
                tools::UPDATE,
                serde_json::json!({
                    "id": created.id.as_str(), "kind": "monthly", "time": "10:00",
                    "days_of_month": [20], "text": "оформить invoice",
                    "items": ["exchange money", "transfer to tbc"]
                }),
            ),
            InMemoryConversationRepository::new(),
            config(),
        );
        let outcome = service
            .handle(OWNER, &chat(), "make that a checklist")
            .await
            .unwrap();
        let ReminderOutcome::Updated {
            reminder: updated, ..
        } = outcome
        else {
            panic!("expected an update, got {outcome:?}");
        };

        assert_eq!(updated.id, created.id, "same reminder");
        assert_eq!(updated.items.len(), 2);
        assert_eq!(updated.items[0].text, "exchange money");
    }

    #[tokio::test]
    async fn adding_a_line_does_not_untick_the_ones_already_done() {
        let service = service(extracting(serde_json::json!({
            "kind": "monthly", "time": "09:00", "days_of_month": [1, 2, 3],
            "text": "monthly payment", "items": ["exchange money", "transfer to tbc"]
        })));
        let created = create(&service, OWNER, "pay monthly").await.unwrap();
        let now = FixedClock::at("2026-03-10T06:00:00Z").now();

        service
            .reminders()
            .complete_items(OWNER, &created.id, &["exchange money".into()], now)
            .await
            .unwrap();

        let updated = service
            .reminders()
            .update(
                OWNER,
                &created.id,
                created.recurrence.clone(),
                "monthly payment".into(),
                vec![
                    TodoItem::new("exchange money"),
                    TodoItem::new("transfer to tbc"),
                    TodoItem::new("call the accountant"),
                ],
                now,
            )
            .await
            .unwrap();

        assert_eq!(updated.items.len(), 3);
        assert!(
            updated.is_done(&updated.items[0], now).unwrap(),
            "the tick survived the edit"
        );
        assert_eq!(updated.outstanding(now).unwrap().len(), 2);
    }

    #[tokio::test]
    async fn editing_a_finished_checklist_does_not_start_it_nagging_again() {
        let service = service(extracting(serde_json::json!({
            "kind": "monthly", "time": "09:00", "days_of_month": [1, 2, 3],
            "text": "monthly payment", "items": ["exchange money"]
        })));
        let created = create(&service, OWNER, "pay monthly").await.unwrap();
        let now = FixedClock::at("2026-03-10T06:00:00Z").now();
        service
            .reminders()
            .complete_items(OWNER, &created.id, &["exchange money".into()], now)
            .await
            .unwrap();

        // Fixing the heading must not resurrect a period that is already finished.
        let updated = service
            .reminders()
            .update(
                OWNER,
                &created.id,
                created.recurrence.clone(),
                "monthly payment (TBC)".into(),
                vec![TodoItem::new("exchange money")],
                now,
            )
            .await
            .unwrap();
        assert!(updated.all_done(now).unwrap());
        assert!(updated.acknowledged_through.is_some(), "still quiet");
    }

    #[tokio::test]
    async fn updating_clears_an_outstanding_acknowledgement() {
        // The new schedule may divide time into different periods, so a watermark from the
        // old one could silence days the user has just asked for.
        let service = service(extracting(monthly_invoice()));
        let created = create(&service, OWNER, "каждый месяц 20").await.unwrap();
        let acked = service
            .reminders()
            .acknowledge(OWNER, &created.id)
            .await
            .unwrap();
        assert!(acked.acknowledged_through.is_some());

        let updated = service
            .reminders()
            .update(
                OWNER,
                &created.id,
                Recurrence::Daily {
                    at: jiff::civil::time(9, 0, 0, 0),
                },
                "оформить invoice".into(),
                Vec::new(),
                FixedClock::at("2026-03-10T06:00:00Z").now(),
            )
            .await
            .unwrap();
        assert_eq!(updated.acknowledged_through, None);
        assert!(updated.is_active());
    }

    #[tokio::test]
    async fn acknowledging_silences_the_period_without_deleting_the_reminder() {
        let service = service(extracting(serde_json::json!({
            "kind": "monthly", "time": "22:30",
            "days_of_month": [20, 21, 22, 23, 24, 25, 26],
            "text": "issue the invoice"
        })));
        let created = create(
            &service,
            OWNER,
            "every day from the 20th to the 26th at 22:30",
        )
        .await
        .unwrap();

        let acked = service
            .reminders()
            .acknowledge(OWNER, &created.id)
            .await
            .unwrap();
        assert!(acked.acknowledged_through.is_some());
        assert!(acked.is_active(), "it comes back, it is not deleted");
        assert!(
            acked.next_fire_at.unwrap() > created.next_fire_at.unwrap(),
            "the next firing moved past the silenced days"
        );
        assert!(
            service
                .reminders()
                .repository
                .get(&created.id)
                .await
                .unwrap()
                .is_some(),
            "still stored"
        );
    }

    #[tokio::test]
    async fn a_listing_leaves_out_what_can_never_fire_again() {
        let service = service(extracting(serde_json::json!({
            "kind": "once", "time": "09:00", "date": "2026-03-15", "text": "call the bank"
        })));
        let created = create(&service, OWNER, "call the bank on the 15th")
            .await
            .unwrap();
        assert_eq!(
            service.reminders().list(OWNER).await.unwrap().len(),
            1,
            "still coming"
        );

        // Deliver it. A one-off has nothing after that.
        let mut spent = created.clone();
        spent
            .mark_fired("2026-03-15T06:00:00Z".parse().unwrap())
            .unwrap();
        service.reminders().repository.put(&spent).await.unwrap();

        assert!(
            service.reminders().list(OWNER).await.unwrap().is_empty(),
            "history does not crowd the listing"
        );
        // But it is still there, so it can still be named and deleted.
        assert_eq!(service.reminders().list_all(OWNER).await.unwrap().len(), 1);
        assert!(
            service
                .reminders()
                .repository
                .get(&created.id)
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn an_acknowledged_repeating_reminder_is_still_listed() {
        // It is quiet, not finished — it comes back next period, so it is still coming.
        let service = service(extracting(monthly_invoice()));
        let created = create(&service, OWNER, "каждый месяц 20").await.unwrap();
        service
            .reminders()
            .acknowledge(OWNER, &created.id)
            .await
            .unwrap();

        let listed = service.reminders().list(OWNER).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].acknowledged_through.is_some());
    }

    #[tokio::test]
    async fn finishing_a_one_off_removes_it_rather_than_leaving_a_tombstone() {
        // A spent one-off can never fire again, so keeping the row would only clutter
        // every listing from then on.
        let service = service(extracting(serde_json::json!({
            "kind": "once", "time": "09:00", "date": "2026-03-15",
            "text": "call the bank"
        })));
        let created = create(&service, OWNER, "call the bank on the 15th")
            .await
            .unwrap();

        let acked = service
            .reminders()
            .acknowledge(OWNER, &created.id)
            .await
            .unwrap();
        assert!(!acked.recurrence.is_recurring());
        assert!(
            service
                .reminders()
                .repository
                .get(&created.id)
                .await
                .unwrap()
                .is_none(),
            "gone from storage"
        );
        assert!(service.reminders().list(OWNER).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn finishing_a_repeating_reminder_keeps_it_for_next_time() {
        let service = service(extracting(monthly_invoice()));
        let created = create(&service, OWNER, "каждый месяц 20").await.unwrap();

        service
            .reminders()
            .acknowledge(OWNER, &created.id)
            .await
            .unwrap();
        let stored = service
            .reminders()
            .repository
            .get(&created.id)
            .await
            .unwrap()
            .unwrap();
        assert!(stored.is_active(), "it comes back next period");
        assert!(stored.acknowledged_through.is_some());
    }

    #[tokio::test]
    async fn ticking_the_last_item_of_a_one_off_checklist_removes_it_too() {
        let service = service(extracting(serde_json::json!({
            "kind": "once", "time": "09:00", "date": "2026-03-15",
            "text": "moving day", "items": ["pack", "hire a van"]
        })));
        let created = create(&service, OWNER, "moving day on the 15th")
            .await
            .unwrap();
        let now = FixedClock::at("2026-03-10T06:00:00Z").now();

        let (_, ticked) = service
            .reminders()
            .complete_items(
                OWNER,
                &created.id,
                &["pack".into(), "hire a van".into()],
                now,
            )
            .await
            .unwrap();
        assert_eq!(ticked.len(), 2);
        assert!(
            service
                .reminders()
                .repository
                .get(&created.id)
                .await
                .unwrap()
                .is_none(),
            "the last tick finished it for good"
        );
    }

    #[tokio::test]
    async fn ticking_only_some_items_leaves_the_reminder_alone() {
        let service = service(extracting(serde_json::json!({
            "kind": "once", "time": "09:00", "date": "2026-03-15",
            "text": "moving day", "items": ["pack", "hire a van"]
        })));
        let created = create(&service, OWNER, "moving day").await.unwrap();
        let now = FixedClock::at("2026-03-10T06:00:00Z").now();

        service
            .reminders()
            .complete_items(OWNER, &created.id, &["pack".into()], now)
            .await
            .unwrap();
        let stored = service
            .reminders()
            .repository
            .get(&created.id)
            .await
            .unwrap()
            .unwrap();
        assert!(stored.is_active(), "one item left, so it still fires");
        assert_eq!(stored.outstanding(now).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn acknowledging_someone_elses_reminder_reports_not_found() {
        let service = service(extracting(monthly_invoice()));
        let created = create(&service, OWNER, "every month on the 20th")
            .await
            .unwrap();
        // Same wording as a missing id, so ids cannot be probed for existence.
        assert!(matches!(
            service.reminders().acknowledge(OTHER, &created.id).await,
            Err(ReminderError::NotFound(_))
        ));
        assert!(matches!(
            service
                .reminders()
                .acknowledge(OWNER, &ReminderId::new("nope"))
                .await,
            Err(ReminderError::NotFound(_))
        ));
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
                serde_json::json!({"kind": "weekly", "time": "18:00", "weekdays": ["friday"],
                                   "text": "отчёт"}),
                Recurrence::Weekly {
                    days: WeekDays::new([Weekday::Friday]).unwrap(),
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
            let reminder = create(&service, OWNER, "что-нибудь").await.unwrap();
            assert_eq!(reminder.recurrence, expected);
            assert!(reminder.is_active());
        }
    }

    #[tokio::test]
    async fn the_turn_carries_the_local_time_on_the_message_not_the_system_prompt() {
        let service = service(extracting(monthly_invoice()));
        create(&service, OWNER, "каждый месяц 20 число")
            .await
            .unwrap();

        let request = &service.llm.requests()[0];

        // The moment rides the user message. Putting it in the system prompt would change
        // that prompt on every turn and cost the provider's prompt cache — the whole reason
        // the two were split (docs/reference-notes.md §2).
        let Message::User { content } = &request.messages[0] else {
            panic!(
                "the turn should open with a user message: {:?}",
                request.messages
            );
        };
        for fact in ["2026-03-10", "10:00", "Tuesday", "Asia/Tbilisi"] {
            assert!(content.contains(fact), "{fact} missing from {content}");
        }
        assert!(
            content.ends_with("каждый месяц 20 число"),
            "the person's own words come last: {content}"
        );

        for volatile in ["2026-03-10", "Tuesday"] {
            assert!(
                !request.system_prompt.contains(volatile),
                "{volatile} must not be in the system prompt: {}",
                request.system_prompt
            );
        }

        // Every action is offered on every turn: the model picks the verb, not the user.
        let offered: Vec<&str> = request.tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            offered,
            vec![
                tools::CREATE,
                tools::UPDATE,
                tools::DELETE,
                tools::COMPLETE,
                tools::ACKNOWLEDGE
            ]
        );
        assert_eq!(request.model, "gpt-5-mini");
    }

    #[tokio::test]
    async fn an_empty_request_is_refused_without_asking_the_model() {
        let service = service(ScriptedLlm::new());
        for request in ["", "   ", "\n"] {
            assert!(matches!(
                create(&service, OWNER, request).await,
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
        // Prose is now an outcome, not a failure: it is how the model asks which of several
        // reminders was meant, and how it says what it still needs.
        let service = service(ScriptedLlm::new().answering("I need to know what time of day."));
        let outcome = service
            .handle(OWNER, &chat(), "напомни про invoice")
            .await
            .unwrap();
        assert_eq!(
            outcome,
            ReminderOutcome::Said("I need to know what time of day.".into())
        );
        assert!(service.reminders().repository.is_empty());
    }

    #[tokio::test]
    async fn an_empty_prose_answer_still_produces_a_usable_message() {
        // A blank answer would otherwise reach the user as an empty message.
        let service = service(ScriptedLlm::new().answering("   "));
        let outcome = service.handle(OWNER, &chat(), "что-то").await.unwrap();
        let ReminderOutcome::Said(message) = outcome else {
            panic!("expected prose, got {outcome:?}");
        };
        assert!(!message.trim().is_empty(), "{message:?}");
    }

    #[tokio::test]
    async fn malformed_function_arguments_are_reported_not_panicked_on() {
        let service = service(ScriptedLlm::new().calling(vec![ToolCall::new(
            "c1",
            tools::CREATE,
            r#"{"kind":"monthly"#,
        )]));
        let err = create(&service, OWNER, "что-то").await.unwrap_err();
        assert!(matches!(err, ReminderError::Unparsable(_)), "{err:?}");
        assert!(service.reminders().repository.is_empty());
    }

    #[tokio::test]
    async fn arguments_missing_a_required_field_are_reported() {
        let service = service(extracting(serde_json::json!({"kind": "daily"})));
        let err = create(&service, OWNER, "что-то").await.unwrap_err();
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
            create(&service, OWNER, "что-то").await,
            Err(ReminderError::NeverFires)
        ));
        assert!(service.reminders().repository.is_empty());
    }

    #[tokio::test]
    async fn a_provider_failure_propagates_rather_than_being_reported_as_unparsable() {
        // The user should be told the model was unreachable, not that their phrasing was bad.
        let service = service(ScriptedLlm::new().failing(LlmError::RateLimited {
            retry_after_secs: Some(30),
        }));
        let err = create(&service, OWNER, "что-то").await.unwrap_err();
        assert!(
            matches!(err, ReminderError::Llm(LlmError::RateLimited { .. })),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn a_listing_is_scoped_to_its_owner() {
        let service = service(extracting(monthly_invoice()).calling(vec![ToolCall::new(
            "c2",
            tools::CREATE,
            monthly_invoice().to_string(),
        )]));
        create(&service, OWNER, "мне").await.unwrap();
        create(&service, OTHER, "им").await.unwrap();

        let mine = service.reminders().list(OWNER).await.unwrap();
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].owner, OWNER);
        assert_eq!(
            service
                .reminders()
                .list(UserId::new(999))
                .await
                .unwrap()
                .len(),
            0
        );
    }

    #[tokio::test]
    async fn deleting_your_own_reminder_removes_it_and_returns_it() {
        let service = service(extracting(monthly_invoice()));
        let created = create(&service, OWNER, "что-то").await.unwrap();

        let deleted = service
            .reminders()
            .delete(OWNER, &created.id)
            .await
            .unwrap();
        assert_eq!(deleted.id, created.id);
        assert!(service.reminders().repository.is_empty());
    }

    #[tokio::test]
    async fn deleting_someone_elses_reminder_reports_not_found_and_leaves_it_alone() {
        // Not a permission error: that would confirm the id exists, and ids are guessable.
        let service = service(extracting(monthly_invoice()));
        let created = create(&service, OWNER, "что-то").await.unwrap();

        let err = service
            .reminders()
            .delete(OTHER, &created.id)
            .await
            .unwrap_err();
        assert!(matches!(err, ReminderError::NotFound(_)), "{err:?}");
        assert!(
            service
                .reminders()
                .repository
                .get(&created.id)
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn deleting_an_unknown_id_reports_not_found() {
        let service = service(ScriptedLlm::new());
        let err = service
            .reminders()
            .delete(OWNER, &ReminderId::new("nope"))
            .await
            .unwrap_err();
        assert!(matches!(err, ReminderError::NotFound(_)), "{err:?}");
    }
}
