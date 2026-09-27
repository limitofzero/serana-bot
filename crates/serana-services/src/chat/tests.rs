//! The loop's own contract, driven by a capability that does nothing but record.
//!
//! These cover what every capability inherits and none of them should have to re-test: the
//! alternation, one result per call, the scoping of histories, and when compaction fires.

use std::sync::Mutex;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;

use serana_domain::conversation::ConversationId;
use serana_domain::conversation_store::ConversationRepository;
use serana_domain::message::{Message, ToolCall, Usage};
use serana_domain::reminder::{TimeZoneName, UserId};
use serana_domain::tool::ToolSpec;
use serana_domain::{CompletionResponse, FinishReason, LlmError, StorageError};
use serana_testkit::{InMemoryConversationRepository, ScriptedLlm};

use super::*;

const OWNER: UserId = UserId::new(1);

fn ts(raw: &str) -> jiff::Timestamp {
    raw.parse().unwrap()
}

fn base() -> ConversationId {
    ConversationId::new("tg:1")
}

#[derive(Debug, thiserror::Error)]
enum FakeError {
    #[error("malformed: {0}")]
    Malformed(String),
    #[error("refused")]
    Refused,
    #[error(transparent)]
    Llm(#[from] LlmError),
    #[error(transparent)]
    Storage(#[from] StorageError),
}

impl TurnError for FakeError {
    fn malformed(reason: String) -> Self {
        Self::Malformed(reason)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Note {
    Said(String),
    Wrote(String),
}

#[derive(Debug, Deserialize, JsonSchema)]
struct WriteArgs {
    /// What to write down.
    line: String,
}

/// A capability that writes a line down, and refuses one particular word so the error path
/// has something to travel along.
struct Notes {
    now: jiff::Timestamp,
    ran: Mutex<Vec<String>>,
}

impl Notes {
    fn new() -> Self {
        Self {
            now: ts("2026-03-10T06:00:00Z"),
            ran: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl Capability for Notes {
    type Outcome = Note;
    type Error = FakeError;

    fn topic(&self) -> &'static str {
        "notes"
    }

    fn instructions(&self) -> &'static str {
        "You write notes."
    }

    fn tools(&self) -> Vec<ToolSpec> {
        vec![ToolSpec::typed::<WriteArgs>("write", "Write a line down.")]
    }

    fn knows(&self, name: &str) -> bool {
        name == "write"
    }

    fn now(&self) -> jiff::Timestamp {
        self.now
    }

    async fn context(&self, _owner: UserId, now: jiff::Timestamp) -> Result<String, FakeError> {
        Ok(format!("[now: {now}]"))
    }

    async fn run(
        &self,
        _owner: UserId,
        _now: jiff::Timestamp,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<Note, FakeError> {
        let args: WriteArgs = serde_json::from_value(arguments)
            .map_err(|e| FakeError::Malformed(format!("the answer was incomplete: {e}")))?;
        if args.line == "no" {
            return Err(FakeError::Refused);
        }
        self.ran.lock().unwrap().push(name.to_owned());
        Ok(Note::Wrote(args.line))
    }

    fn said(&self, words: String) -> Note {
        Note::Said(words)
    }

    fn summarise(&self, outcome: &Note) -> String {
        match outcome {
            Note::Said(words) => words.clone(),
            Note::Wrote(line) => format!("{{\"ok\":\"wrote\",\"line\":{line:?}}}"),
        }
    }
}

fn config() -> ChatConfig {
    ChatConfig {
        model: "gpt-5-mini".into(),
        default_timezone: TimeZoneName::new("Asia/Tbilisi"),
        summary_model: None,
        compact_above_tokens: DEFAULT_COMPACT_ABOVE_TOKENS,
        temperature: None,
    }
}

type Session = Chat<Notes, std::sync::Arc<ScriptedLlm>, InMemoryConversationRepository>;

fn chat(llm: ScriptedLlm) -> (Session, std::sync::Arc<ScriptedLlm>) {
    let llm = std::sync::Arc::new(llm);
    let chat = Chat::new(
        Notes::new(),
        std::sync::Arc::clone(&llm),
        InMemoryConversationRepository::new(),
        config(),
    );
    (chat, llm)
}

fn writing(line: &str) -> ScriptedLlm {
    ScriptedLlm::new().calling(vec![ToolCall::new(
        "c1",
        "write",
        serde_json::json!({ "line": line }).to_string(),
    )])
}

#[tokio::test]
async fn a_tool_the_model_chose_is_run_and_its_outcome_returned() {
    let (chat, llm) = chat(writing("milk"));
    let outcome = chat.handle(OWNER, &base(), "note milk").await.unwrap();
    assert_eq!(outcome, Note::Wrote("milk".into()));
    assert_eq!(llm.remaining(), 0);
}

#[tokio::test]
async fn the_per_turn_context_rides_the_user_message_rather_than_the_system_prompt() {
    // The prompt-caching invariant: a system prompt that moves invalidates the provider's
    // cache and re-bills the whole history every turn.
    let (chat, llm) = chat(writing("milk"));
    chat.handle(OWNER, &base(), "note milk").await.unwrap();

    let request = &llm.requests()[0];
    assert_eq!(&*request.system_prompt, "You write notes.");
    let Message::User { content } = &request.messages[0] else {
        panic!("the turn opens with a user message");
    };
    assert!(content.starts_with("[now: 2026-03-10"), "{content}");
    assert!(content.ends_with("note milk"), "{content}");
}

#[tokio::test]
async fn prose_is_recorded_as_an_assistant_message_so_the_next_turn_is_legal() {
    let (chat, _) = chat(ScriptedLlm::new().answering("which one?"));
    let outcome = chat.handle(OWNER, &base(), "note it").await.unwrap();
    assert_eq!(outcome, Note::Said("which one?".into()));

    let history = chat.stored(&base()).await.unwrap().unwrap();
    assert_eq!(
        history.messages().last().unwrap(),
        &Message::assistant("which one?")
    );
}

#[tokio::test]
async fn an_answer_with_nothing_in_it_still_says_something() {
    let (chat, _) = chat(ScriptedLlm::new().answering("   "));
    let Note::Said(words) = chat.handle(OWNER, &base(), "note it").await.unwrap() else {
        panic!("prose");
    };
    assert!(!words.trim().is_empty(), "{words}");
}

#[tokio::test]
async fn every_tool_call_gets_exactly_one_result() {
    // Two calls asked for, one run. Leaving the second open would start the next turn with
    // `tool -> user`, which providers reject.
    let llm = ScriptedLlm::new().calling(vec![
        ToolCall::new("c1", "write", serde_json::json!({"line": "a"}).to_string()),
        ToolCall::new("c2", "write", serde_json::json!({"line": "b"}).to_string()),
    ]);
    let (chat, _) = chat(llm);
    chat.handle(OWNER, &base(), "two things").await.unwrap();

    let history = chat.stored(&base()).await.unwrap().unwrap();
    let results: Vec<&Message> = history
        .messages()
        .iter()
        .filter(|m| matches!(m, Message::Tool { .. }))
        .collect();
    assert_eq!(results.len(), 2, "{:?}", history.messages());
    assert!(history.pending_tool_calls().is_empty());
}

#[tokio::test]
async fn a_tool_name_we_do_not_offer_is_never_dispatched() {
    // A hallucinated name reaching `run` would be an action nobody can check.
    let llm = ScriptedLlm::new().responding(CompletionResponse {
        content: Some("I cannot do that.".into()),
        tool_calls: vec![ToolCall::new("c1", "drop_everything", "{}")],
        finish_reason: FinishReason::ToolCalls,
        usage: Usage::default(),
    });
    let (chat, _) = chat(llm);
    assert_eq!(
        chat.handle(OWNER, &base(), "go").await.unwrap(),
        Note::Said("I cannot do that.".into())
    );
}

#[tokio::test]
async fn arguments_that_do_not_fit_are_reported_rather_than_dispatched() {
    let llm = ScriptedLlm::new().calling(vec![ToolCall::new("c1", "write", "{\"line\": 7}")]);
    let (chat, _) = chat(llm);
    assert!(matches!(
        chat.handle(OWNER, &base(), "go").await,
        Err(FakeError::Malformed(_))
    ));
}

#[tokio::test]
async fn arguments_that_are_not_json_at_all_are_reported() {
    let llm = ScriptedLlm::new().calling(vec![ToolCall::new("c1", "write", "{not json")]);
    let (chat, _) = chat(llm);
    assert!(matches!(
        chat.handle(OWNER, &base(), "go").await,
        Err(FakeError::Malformed(_))
    ));
}

#[tokio::test]
async fn an_empty_request_costs_nothing() {
    let (chat, llm) = chat(ScriptedLlm::new());
    for request in ["", "   \n "] {
        assert!(matches!(
            chat.handle(OWNER, &base(), request).await,
            Err(FakeError::Malformed(_))
        ));
    }
    assert_eq!(llm.call_count(), 0, "no tokens spent on an empty request");
}

#[tokio::test]
async fn a_capability_failure_leaves_the_history_untouched() {
    // The turn is abandoned before anything is saved, so the next one does not open on a
    // user message with no answer under it.
    let (chat, _) = chat(writing("no"));
    assert!(matches!(
        chat.handle(OWNER, &base(), "note no").await,
        Err(FakeError::Refused)
    ));
    assert!(chat.stored(&base()).await.unwrap().is_none());
}

#[tokio::test]
async fn each_capability_keeps_its_own_history_under_the_caller_s_id() {
    // Two capabilities sharing an id would mean whichever spoke first deciding the other
    // one's system prompt, which is stored with the conversation and never changes.
    let (chat, _) = chat(writing("milk"));
    chat.handle(OWNER, &base(), "note milk").await.unwrap();

    let store = &chat.conversations;
    assert!(store.load(&base()).await.unwrap().is_none());
    assert!(
        store
            .load(&ConversationId::new("tg:1:notes"))
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn a_conversation_is_compacted_once_the_provider_says_it_has_grown_too_long() {
    let llm = ScriptedLlm::new()
        .responding(CompletionResponse {
            content: Some("noted".into()),
            tool_calls: Vec::new(),
            finish_reason: FinishReason::Stop,
            usage: Usage {
                prompt_tokens: DEFAULT_COMPACT_ABOVE_TOKENS + 1,
                ..Usage::default()
            },
        })
        .answering("they asked about milk");
    let (chat, llm) = chat(llm);
    chat.handle(OWNER, &base(), "note milk").await.unwrap();

    let history = chat.stored(&base()).await.unwrap().unwrap();
    assert_eq!(history.messages().len(), 2, "{:?}", history.messages());
    assert!(
        matches!(&history.messages()[0], Message::User { content } if content.contains("milk")),
        "{:?}",
        history.messages()
    );
    assert_eq!(history.system_prompt(), "You write notes.");
    assert_eq!(llm.remaining(), 0, "the summary call happened");
}

#[tokio::test]
async fn a_conversation_under_the_threshold_is_left_alone() {
    let (chat, llm) = chat(writing("milk"));
    chat.handle(OWNER, &base(), "note milk").await.unwrap();
    assert_eq!(llm.call_count(), 1, "no summary call");
    let history = chat.stored(&base()).await.unwrap().unwrap();
    assert_eq!(history.messages().len(), 3);
}

#[tokio::test]
async fn compacting_a_conversation_that_never_happened_folds_nothing_up() {
    let (session, _) = chat(ScriptedLlm::new());
    assert!(!session.compact_conversation(&base()).await.unwrap());
}

#[tokio::test]
async fn compacting_on_request_reports_that_there_was_something_to_fold_up() {
    let (session, _) = chat(writing("milk").answering("they asked about milk"));
    session.handle(OWNER, &base(), "note milk").await.unwrap();
    assert!(session.compact_conversation(&base()).await.unwrap());
}

#[tokio::test]
async fn the_system_prompt_survives_compaction_byte_for_byte() {
    let (chat, llm) = chat(
        writing("milk")
            .answering("they asked about milk")
            .calling(vec![ToolCall::new(
                "c2",
                "write",
                serde_json::json!({"line": "bread"}).to_string(),
            )]),
    );
    chat.handle(OWNER, &base(), "note milk").await.unwrap();
    chat.compact_conversation(&base()).await.unwrap();
    chat.handle(OWNER, &base(), "note bread").await.unwrap();

    let requests = llm.requests();
    let prompts: Vec<&str> = requests.iter().map(|r| r.system_prompt.as_ref()).collect();
    assert_eq!(prompts[0], prompts[2], "the turn prompt never moves");
}
