//! The conversation's atoms: messages, tool calls and token accounting.
//!
//! These types are storage and wire agnostic. Adapters own their own DTOs and translate;
//! the serde derives here exist so repositories can persist a conversation, not so an
//! adapter can `serde_json::to_value` a message straight onto the wire.

use serde::{Deserialize, Serialize};

/// Identifier a provider assigns to one tool invocation request.
///
/// Providers do not guarantee these are unique within a batch — models have been observed
/// emitting duplicates — so uniquification happens during validation, before anything
/// downstream indexes by this value.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ToolCallId(String);

impl ToolCallId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ToolCallId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A model's request to invoke one tool.
///
/// `arguments` stays a raw string rather than a parsed `serde_json::Value` on purpose.
/// Models emit malformed JSON, and routers that rewrite a `length` finish reason into
/// `tool_calls` emit arguments truncated mid-token. Parsing must therefore be a fallible
/// step the validator owns, not a precondition of constructing this type — otherwise the
/// only place left to report the failure is a deserialization error far from the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: ToolCallId,
    pub name: String,
    pub arguments: String,
}

impl ToolCall {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        Self {
            id: ToolCallId::new(id),
            name: name.into(),
            arguments: arguments.into(),
        }
    }

    /// Parse `arguments` as a JSON object.
    ///
    /// An empty or whitespace-only string means "no arguments" and yields an empty object;
    /// several providers emit `""` rather than `"{}"` for a zero-argument tool.
    pub fn parse_arguments(&self) -> Result<serde_json::Value, serde_json::Error> {
        if self.arguments.trim().is_empty() {
            return Ok(serde_json::Value::Object(serde_json::Map::new()));
        }
        serde_json::from_str(&self.arguments)
    }
}

/// One entry in a conversation.
///
/// There is no `System` variant: the system prompt is byte-stable for the life of a
/// conversation and is held separately by [`crate::conversation::Conversation`], so it
/// cannot be appended, reordered or rewritten by accident.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum Message {
    User {
        content: String,
    },
    Assistant {
        content: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ToolCall>,
    },
    Tool {
        tool_call_id: ToolCallId,
        name: String,
        content: String,
    },
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self::User {
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::Assistant {
            content: Some(content.into()),
            tool_calls: Vec::new(),
        }
    }

    pub fn assistant_tool_calls(calls: Vec<ToolCall>) -> Self {
        Self::Assistant {
            content: None,
            tool_calls: calls,
        }
    }

    pub fn tool_result(call: &ToolCall, content: impl Into<String>) -> Self {
        Self::Tool {
            tool_call_id: call.id.clone(),
            name: call.name.clone(),
            content: content.into(),
        }
    }

    /// Tool calls this message requests, empty for every variant but `Assistant`.
    pub fn tool_calls(&self) -> &[ToolCall] {
        match self {
            Self::Assistant { tool_calls, .. } => tool_calls,
            _ => &[],
        }
    }

    /// Wire role name, for adapters and for error messages.
    pub fn role(&self) -> &'static str {
        match self {
            Self::User { .. } => "user",
            Self::Assistant { .. } => "assistant",
            Self::Tool { .. } => "tool",
        }
    }
}

/// Token accounting for one provider call.
///
/// `cached_prompt_tokens` is tracked separately because it is the only way to tell whether
/// the prompt-cache invariant is actually holding: a conversation whose cached count keeps
/// resetting to zero has something rewriting its prefix.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cached_prompt_tokens: u64,
}

impl Usage {
    pub fn total(&self) -> u64 {
        self.prompt_tokens + self.completion_tokens
    }
}

impl std::ops::Add for Usage {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        Self {
            prompt_tokens: self.prompt_tokens + rhs.prompt_tokens,
            completion_tokens: self.completion_tokens + rhs.completion_tokens,
            cached_prompt_tokens: self.cached_prompt_tokens + rhs.cached_prompt_tokens,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_arguments_parse_as_an_empty_object() {
        for raw in ["", "   ", "\n"] {
            let call = ToolCall::new("c1", "now", raw);
            let parsed = call.parse_arguments().expect("empty args are valid");
            assert_eq!(parsed, serde_json::json!({}));
        }
    }

    #[test]
    fn well_formed_arguments_parse() {
        let call = ToolCall::new("c1", "echo", r#"{"text":"hi"}"#);
        assert_eq!(
            call.parse_arguments().unwrap(),
            serde_json::json!({"text": "hi"})
        );
    }

    #[test]
    fn truncated_arguments_surface_as_a_parse_error_not_a_panic() {
        let call = ToolCall::new("c1", "echo", r#"{"text":"hi"#);
        assert!(call.parse_arguments().is_err());
    }

    #[test]
    fn only_assistant_messages_carry_tool_calls() {
        let call = ToolCall::new("c1", "now", "{}");
        assert_eq!(
            Message::assistant_tool_calls(vec![call]).tool_calls().len(),
            1
        );
        assert!(Message::user("hi").tool_calls().is_empty());
        assert!(Message::assistant("hi").tool_calls().is_empty());
    }

    #[test]
    fn tool_result_inherits_the_call_identity() {
        let call = ToolCall::new("c1", "now", "{}");
        let Message::Tool {
            tool_call_id,
            name,
            content,
        } = Message::tool_result(&call, "Friday")
        else {
            panic!("expected a tool message");
        };
        assert_eq!(tool_call_id, call.id);
        assert_eq!(name, "now");
        assert_eq!(content, "Friday");
    }

    #[test]
    fn messages_round_trip_through_serde() {
        let original = vec![
            Message::user("hi"),
            Message::assistant_tool_calls(vec![ToolCall::new("c1", "now", "{}")]),
            Message::Tool {
                tool_call_id: ToolCallId::new("c1"),
                name: "now".into(),
                content: "Friday".into(),
            },
            Message::assistant("It is Friday."),
        ];
        let json = serde_json::to_string(&original).unwrap();
        assert_eq!(
            serde_json::from_str::<Vec<Message>>(&json).unwrap(),
            original
        );
    }

    #[test]
    fn usage_adds_componentwise() {
        let a = Usage {
            prompt_tokens: 10,
            completion_tokens: 2,
            cached_prompt_tokens: 8,
        };
        let b = Usage {
            prompt_tokens: 5,
            completion_tokens: 3,
            cached_prompt_tokens: 5,
        };
        let sum = a + b;
        assert_eq!(sum.prompt_tokens, 15);
        assert_eq!(sum.completion_tokens, 5);
        assert_eq!(sum.cached_prompt_tokens, 13);
        assert_eq!(sum.total(), 20);
    }
}
