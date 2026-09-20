//! Chat-completions wire types.
//!
//! These are deliberately separate from the domain types. The domain models a conversation
//! the way we want to reason about it; this models the bytes a provider accepts. Keeping
//! them apart is what lets the domain forbid a `System` message in the history while the
//! wire format still requires one at the front.
//!
//! Deserialisation is lenient on purpose: OpenAI-compatible gateways omit fields, send
//! `null` where the spec says array, and add fields we do not care about. Being strict here
//! turns a provider quirk into a hard failure for the user.

use serde::{Deserialize, Serialize};

use serana_domain::llm::{CompletionRequest, FinishReason};
use serana_domain::message::{Message, ToolCall, ToolCallId, Usage};

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ChatRequest {
    pub model: String,
    pub messages: Vec<WireMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<WireTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
}

impl From<&CompletionRequest> for ChatRequest {
    fn from(request: &CompletionRequest) -> Self {
        // The system prompt is prepended here, at the boundary, and nowhere else. It never
        // exists as an element of the conversation's history.
        let mut messages = Vec::with_capacity(request.messages.len() + 1);
        messages.push(WireMessage::System {
            content: request.system_prompt.to_string(),
        });
        messages.extend(request.messages.iter().map(WireMessage::from));

        Self {
            model: request.model.clone(),
            messages,
            tools: request.tools.iter().map(WireTool::from).collect(),
            temperature: request.temperature,
            max_tokens: request.max_tokens,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub(crate) enum WireMessage {
    System {
        content: String,
    },
    User {
        content: String,
    },
    Assistant {
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<WireToolCall>,
    },
    Tool {
        content: String,
        tool_call_id: String,
    },
}

impl From<&Message> for WireMessage {
    fn from(message: &Message) -> Self {
        match message {
            Message::User { content } => Self::User {
                content: content.clone(),
            },
            Message::Assistant {
                content,
                tool_calls,
            } => Self::Assistant {
                content: content.clone(),
                tool_calls: tool_calls.iter().map(WireToolCall::from).collect(),
            },
            Message::Tool {
                tool_call_id,
                content,
                ..
            } => Self::Tool {
                content: content.clone(),
                tool_call_id: tool_call_id.to_string(),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WireToolCall {
    #[serde(default)]
    pub id: String,
    #[serde(rename = "type", default = "function_kind")]
    pub kind: String,
    pub function: WireFunction,
}

fn function_kind() -> String {
    "function".to_owned()
}

impl From<&ToolCall> for WireToolCall {
    fn from(call: &ToolCall) -> Self {
        Self {
            id: call.id.to_string(),
            kind: function_kind(),
            function: WireFunction {
                name: call.name.clone(),
                arguments: call.arguments.clone(),
            },
        }
    }
}

impl From<WireToolCall> for ToolCall {
    fn from(call: WireToolCall) -> Self {
        Self {
            id: ToolCallId::new(call.id),
            name: call.function.name,
            arguments: call.function.arguments,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WireFunction {
    #[serde(default)]
    pub name: String,
    /// A JSON string, not an object. Kept as text all the way into the domain because
    /// models emit malformed and truncated values here.
    #[serde(default)]
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct WireTool {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: WireToolFunction,
}

impl From<&serana_domain::tool::ToolSpec> for WireTool {
    fn from(spec: &serana_domain::tool::ToolSpec) -> Self {
        Self {
            kind: "function",
            function: WireToolFunction {
                name: spec.name.clone(),
                description: spec.description.clone(),
                parameters: spec.parameters.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct WireToolFunction {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ChatResponse {
    #[serde(default)]
    pub choices: Vec<WireChoice>,
    #[serde(default)]
    pub usage: Option<WireUsage>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WireChoice {
    pub message: WireResponseMessage,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WireResponseMessage {
    #[serde(default)]
    pub content: Option<String>,
    /// `Option` rather than `#[serde(default)]` on a `Vec`, because gateways send an
    /// explicit `null` here and a bare default does not cover that.
    #[serde(default)]
    pub tool_calls: Option<Vec<WireToolCall>>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct WireUsage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub prompt_tokens_details: Option<WirePromptDetails>,
}

impl From<WireUsage> for Usage {
    fn from(usage: WireUsage) -> Self {
        Self {
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            cached_prompt_tokens: usage
                .prompt_tokens_details
                .map(|d| d.cached_tokens)
                .unwrap_or_default(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct WirePromptDetails {
    #[serde(default)]
    pub cached_tokens: u64,
}

pub(crate) fn finish_reason(raw: Option<String>) -> FinishReason {
    match raw.as_deref() {
        Some("stop") => FinishReason::Stop,
        Some("tool_calls") | Some("function_call") => FinishReason::ToolCalls,
        Some("length") | Some("max_tokens") => FinishReason::Length,
        Some("content_filter") => FinishReason::ContentFilter,
        Some(other) => FinishReason::Other(other.to_owned()),
        // Absent means the provider did not say. Treat it as a normal stop rather than
        // inventing a failure: the payload already tells us whether tools were requested.
        None => FinishReason::Stop,
    }
}

/// The error body OpenAI-compatible providers return alongside a non-2xx status.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct WireErrorEnvelope {
    #[serde(default)]
    pub error: Option<WireError>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct WireError {
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
}
