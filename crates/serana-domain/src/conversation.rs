//! The conversation aggregate: the message history plus the invariants that keep it
//! acceptable to a provider.
//!
//! Two rules from the reference implementation are enforced here rather than left to
//! review, because violating either produces a provider-side rejection far from the code
//! that caused it (see `docs/reference-notes.md`, sections 2 and 3):
//!
//! 1. **The system prompt is byte-stable for the life of the conversation.** It is held
//!    outside `messages` and there is no API that mutates it. Anything that must reach the
//!    model mid-conversation rides a user message or a tool result.
//! 2. **Strict role alternation.** Never two same-role messages in a row, and every
//!    `tool_call` gets exactly one matching tool result — including on abort paths, which
//!    is what [`Conversation::close_open_tool_calls`] exists for.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::message::{Message, ToolCall, ToolCallId};

/// Identifier for one conversation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConversationId(String);

impl ConversationId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ConversationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Why an append was refused.
///
/// Every variant is a programming error in the caller, not a model failure: the turn loop
/// is responsible for never reaching them. They are values rather than panics so the
/// services layer can assert on them in tests.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AlternationError {
    #[error("cannot append a {attempted} message after a {tail} message")]
    ConsecutiveRoles {
        tail: &'static str,
        attempted: &'static str,
    },

    #[error("cannot open a conversation with an assistant message")]
    AssistantWithoutUser,

    #[error("{pending} tool call(s) are still awaiting results")]
    PendingToolCalls { pending: usize },

    #[error("no tool call is awaiting a result")]
    NoToolCallAwaitingResult,

    #[error("no pending tool call with id {0}")]
    UnknownToolCallId(ToolCallId),

    #[error("tool call {0} already has a result")]
    DuplicateToolResult(ToolCallId),
}

/// What the conversation will accept next, derived from its tail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tail {
    /// Nothing appended yet. Only a user message is legal.
    Empty,
    /// Last message is from the user. Only an assistant message is legal.
    User,
    /// Last message is a plain assistant answer. Only a user message is legal.
    Assistant,
    /// An assistant message requested tools and some results are missing. Only tool
    /// results for the listed ids are legal.
    AwaitingToolResults(Vec<ToolCallId>),
    /// Every requested tool call has a result. Both a user and an assistant message are
    /// legal — `assistant(tool_calls) -> tool -> user` is accepted by every provider path.
    ToolResultsComplete,
}

/// A conversation: an immutable system prompt plus an append-only, role-alternating history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    id: ConversationId,
    /// Never mutated after construction. `Arc<str>` so cloning a conversation for a request
    /// cannot accidentally deep-copy — and, more importantly, cannot diverge.
    system_prompt: Arc<str>,
    messages: Vec<Message>,
    created_at: jiff::Timestamp,
}

impl Conversation {
    /// `created_at` is passed in rather than read from the system clock so the domain stays
    /// pure; callers take it from [`crate::clock::Clock`].
    pub fn new(
        id: ConversationId,
        system_prompt: impl Into<Arc<str>>,
        created_at: jiff::Timestamp,
    ) -> Self {
        Self {
            id,
            system_prompt: system_prompt.into(),
            messages: Vec::new(),
            created_at,
        }
    }

    pub fn id(&self) -> &ConversationId {
        &self.id
    }

    /// The system prompt. There is deliberately no mutable accessor.
    pub fn system_prompt(&self) -> &str {
        &self.system_prompt
    }

    pub fn created_at(&self) -> jiff::Timestamp {
        self.created_at
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// What the conversation will accept next.
    pub fn tail(&self) -> Tail {
        let Some(last) = self.messages.last() else {
            return Tail::Empty;
        };
        match last {
            Message::User { .. } => Tail::User,
            Message::Assistant { tool_calls, .. } if tool_calls.is_empty() => Tail::Assistant,
            Message::Assistant { .. } | Message::Tool { .. } => {
                let pending = self.pending_tool_calls();
                if pending.is_empty() {
                    Tail::ToolResultsComplete
                } else {
                    Tail::AwaitingToolResults(pending)
                }
            }
        }
    }

    /// Tool call ids from the most recent assistant request that have no result yet.
    pub fn pending_tool_calls(&self) -> Vec<ToolCallId> {
        let Some(round_start) = self.last_tool_round_start() else {
            return Vec::new();
        };
        let requested = self.messages[round_start].tool_calls();
        let answered: Vec<&ToolCallId> = self.messages[round_start + 1..]
            .iter()
            .filter_map(|m| match m {
                Message::Tool { tool_call_id, .. } => Some(tool_call_id),
                _ => None,
            })
            .collect();
        requested
            .iter()
            .filter(|c| !answered.contains(&&c.id))
            .map(|c| c.id.clone())
            .collect()
    }

    /// Index of the assistant message that opened the current tool round, if the tail is
    /// still inside one.
    fn last_tool_round_start(&self) -> Option<usize> {
        let mut idx = self.messages.len().checked_sub(1)?;
        loop {
            match &self.messages[idx] {
                Message::Assistant { tool_calls, .. } if !tool_calls.is_empty() => {
                    return Some(idx);
                }
                Message::Tool { .. } => idx = idx.checked_sub(1)?,
                _ => return None,
            }
        }
    }

    pub fn push_user(&mut self, content: impl Into<String>) -> Result<(), AlternationError> {
        match self.tail() {
            Tail::Empty | Tail::Assistant | Tail::ToolResultsComplete => {
                self.messages.push(Message::user(content));
                Ok(())
            }
            Tail::User => Err(AlternationError::ConsecutiveRoles {
                tail: "user",
                attempted: "user",
            }),
            Tail::AwaitingToolResults(pending) => Err(AlternationError::PendingToolCalls {
                pending: pending.len(),
            }),
        }
    }

    /// Append a plain assistant answer.
    pub fn push_assistant(&mut self, content: impl Into<String>) -> Result<(), AlternationError> {
        self.push_assistant_message(Message::assistant(content))
    }

    /// Append an assistant message requesting tools, opening a tool round.
    pub fn push_tool_calls(&mut self, calls: Vec<ToolCall>) -> Result<(), AlternationError> {
        self.push_assistant_message(Message::assistant_tool_calls(calls))
    }

    fn push_assistant_message(&mut self, message: Message) -> Result<(), AlternationError> {
        match self.tail() {
            Tail::User | Tail::ToolResultsComplete => {
                self.messages.push(message);
                Ok(())
            }
            Tail::Empty => Err(AlternationError::AssistantWithoutUser),
            Tail::Assistant => Err(AlternationError::ConsecutiveRoles {
                tail: "assistant",
                attempted: "assistant",
            }),
            Tail::AwaitingToolResults(pending) => Err(AlternationError::PendingToolCalls {
                pending: pending.len(),
            }),
        }
    }

    /// Answer one outstanding tool call.
    pub fn push_tool_result(
        &mut self,
        id: &ToolCallId,
        content: impl Into<String>,
    ) -> Result<(), AlternationError> {
        let Some(round_start) = self.last_tool_round_start() else {
            return Err(AlternationError::NoToolCallAwaitingResult);
        };
        let call = self.messages[round_start]
            .tool_calls()
            .iter()
            .find(|c| &c.id == id)
            .cloned()
            .ok_or_else(|| AlternationError::UnknownToolCallId(id.clone()))?;

        if !self.pending_tool_calls().contains(id) {
            return Err(AlternationError::DuplicateToolResult(id.clone()));
        }

        self.messages.push(Message::tool_result(&call, content));
        Ok(())
    }

    /// Answer every outstanding tool call with `reason`, so the tail is not left mid-round.
    ///
    /// Abort paths — interrupts, budget exhaustion, a partial exit after repeated invalid
    /// tool names — must call this before returning. A conversation persisted with an open
    /// tool round resumes as `tool -> user`, which providers reject.
    ///
    /// Returns how many results were synthesised.
    pub fn close_open_tool_calls(&mut self, reason: &str) -> usize {
        let pending = self.pending_tool_calls();
        for id in &pending {
            // Cannot fail: every id came from `pending_tool_calls`.
            let _ = self.push_tool_result(id, reason);
        }
        pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conversation() -> Conversation {
        Conversation::new(
            ConversationId::new("c"),
            "You are Serana.",
            jiff::Timestamp::UNIX_EPOCH,
        )
    }

    fn call(id: &str) -> ToolCall {
        ToolCall::new(id, "now", "{}")
    }

    #[test]
    fn a_new_conversation_holds_its_prompt_outside_the_history() {
        let c = conversation();
        assert_eq!(c.system_prompt(), "You are Serana.");
        assert!(c.is_empty());
        assert_eq!(c.tail(), Tail::Empty);
    }

    #[test]
    fn a_plain_exchange_alternates() {
        let mut c = conversation();
        c.push_user("hi").unwrap();
        assert_eq!(c.tail(), Tail::User);
        c.push_assistant("hello").unwrap();
        assert_eq!(c.tail(), Tail::Assistant);
        c.push_user("again").unwrap();
        assert_eq!(c.len(), 3);
    }

    #[test]
    fn consecutive_same_roles_are_refused() {
        let mut c = conversation();
        c.push_user("hi").unwrap();
        assert_eq!(
            c.push_user("hi again"),
            Err(AlternationError::ConsecutiveRoles {
                tail: "user",
                attempted: "user"
            })
        );
        c.push_assistant("hello").unwrap();
        assert_eq!(
            c.push_assistant("hello again"),
            Err(AlternationError::ConsecutiveRoles {
                tail: "assistant",
                attempted: "assistant"
            })
        );
    }

    #[test]
    fn a_conversation_cannot_open_with_an_assistant_message() {
        let mut c = conversation();
        assert_eq!(
            c.push_assistant("hi"),
            Err(AlternationError::AssistantWithoutUser)
        );
        assert_eq!(
            c.push_tool_calls(vec![call("c1")]),
            Err(AlternationError::AssistantWithoutUser)
        );
    }

    #[test]
    fn a_tool_round_reports_every_unanswered_call() {
        let mut c = conversation();
        c.push_user("hi").unwrap();
        c.push_tool_calls(vec![call("c1"), call("c2")]).unwrap();
        assert_eq!(
            c.tail(),
            Tail::AwaitingToolResults(vec![ToolCallId::new("c1"), ToolCallId::new("c2")])
        );
        c.push_tool_result(&ToolCallId::new("c1"), "Friday")
            .unwrap();
        assert_eq!(c.pending_tool_calls(), vec![ToolCallId::new("c2")]);
        c.push_tool_result(&ToolCallId::new("c2"), "Friday")
            .unwrap();
        assert_eq!(c.tail(), Tail::ToolResultsComplete);
    }

    #[test]
    fn nothing_but_a_tool_result_is_accepted_while_calls_are_open() {
        let mut c = conversation();
        c.push_user("hi").unwrap();
        c.push_tool_calls(vec![call("c1")]).unwrap();
        assert_eq!(
            c.push_user("interrupt"),
            Err(AlternationError::PendingToolCalls { pending: 1 })
        );
        assert_eq!(
            c.push_assistant("answer"),
            Err(AlternationError::PendingToolCalls { pending: 1 })
        );
    }

    #[test]
    fn tool_results_are_refused_outside_a_round_and_for_unknown_or_repeated_ids() {
        let mut c = conversation();
        c.push_user("hi").unwrap();
        assert_eq!(
            c.push_tool_result(&ToolCallId::new("c1"), "x"),
            Err(AlternationError::NoToolCallAwaitingResult)
        );

        c.push_tool_calls(vec![call("c1")]).unwrap();
        assert_eq!(
            c.push_tool_result(&ToolCallId::new("nope"), "x"),
            Err(AlternationError::UnknownToolCallId(ToolCallId::new("nope")))
        );

        c.push_tool_result(&ToolCallId::new("c1"), "Friday")
            .unwrap();
        assert_eq!(
            c.push_tool_result(&ToolCallId::new("c1"), "Friday"),
            Err(AlternationError::DuplicateToolResult(ToolCallId::new("c1")))
        );
    }

    #[test]
    fn user_may_follow_a_completed_tool_round() {
        // `assistant(tool_calls) -> tool -> user` is the one legal three-step.
        let mut c = conversation();
        c.push_user("hi").unwrap();
        c.push_tool_calls(vec![call("c1")]).unwrap();
        c.push_tool_result(&ToolCallId::new("c1"), "Friday")
            .unwrap();
        c.push_user("thanks").unwrap();
        assert_eq!(c.tail(), Tail::User);
    }

    #[test]
    fn assistant_may_also_follow_a_completed_tool_round() {
        let mut c = conversation();
        c.push_user("hi").unwrap();
        c.push_tool_calls(vec![call("c1")]).unwrap();
        c.push_tool_result(&ToolCallId::new("c1"), "Friday")
            .unwrap();
        c.push_assistant("It is Friday.").unwrap();
        assert_eq!(c.tail(), Tail::Assistant);
    }

    #[test]
    fn closing_an_open_round_makes_the_conversation_resumable() {
        let mut c = conversation();
        c.push_user("hi").unwrap();
        c.push_tool_calls(vec![call("c1"), call("c2")]).unwrap();
        c.push_tool_result(&ToolCallId::new("c1"), "Friday")
            .unwrap();

        assert_eq!(c.close_open_tool_calls("Interrupted."), 1);
        assert_eq!(c.tail(), Tail::ToolResultsComplete);
        assert!(c.pending_tool_calls().is_empty());
        c.push_user("still there?").unwrap();
    }

    #[test]
    fn closing_a_conversation_with_no_open_round_is_a_no_op() {
        let mut c = conversation();
        c.push_user("hi").unwrap();
        c.push_assistant("hello").unwrap();
        assert_eq!(c.close_open_tool_calls("Interrupted."), 0);
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn consecutive_tool_rounds_only_consider_the_latest() {
        let mut c = conversation();
        c.push_user("hi").unwrap();
        c.push_tool_calls(vec![call("c1")]).unwrap();
        c.push_tool_result(&ToolCallId::new("c1"), "Friday")
            .unwrap();
        c.push_tool_calls(vec![call("c2")]).unwrap();
        assert_eq!(c.pending_tool_calls(), vec![ToolCallId::new("c2")]);
        assert_eq!(
            c.push_tool_result(&ToolCallId::new("c1"), "again"),
            Err(AlternationError::UnknownToolCallId(ToolCallId::new("c1")))
        );
    }

    #[test]
    fn the_system_prompt_never_appears_in_the_history() {
        let mut c = conversation();
        c.push_user("hi").unwrap();
        c.push_assistant("hello").unwrap();
        assert!(c.messages().iter().all(|m| m.role() != "system"));
    }
}
