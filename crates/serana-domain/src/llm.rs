//! The LLM port: one stateless call in, one response out.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::LlmError;
use crate::message::{Message, ToolCall, Usage};
use crate::tool::ToolSpec;

/// One provider call.
///
/// `system_prompt` is separate from `messages` rather than being the first element, so the
/// byte-stability invariant survives the trip to the adapter: there is no position in
/// `messages` at which a system message could be inserted, reordered or rewritten. The
/// adapter prepends it when building the wire payload.
#[derive(Debug, Clone)]
pub struct CompletionRequest {
    pub model: String,
    pub system_prompt: Arc<str>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

impl CompletionRequest {
    pub fn new(model: impl Into<String>, system_prompt: Arc<str>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            system_prompt,
            messages,
            tools: Vec::new(),
            temperature: None,
            max_tokens: None,
        }
    }

    pub fn with_tools(mut self, tools: Vec<ToolSpec>) -> Self {
        self.tools = tools;
        self
    }

    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }
}

/// Why the provider stopped generating.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    /// The model finished its answer.
    Stop,
    /// The model wants tools invoked.
    ToolCalls,
    /// Output hit the token limit. Tool call arguments from a `Length` finish are
    /// potentially truncated mid-token and must not be dispatched.
    Length,
    ContentFilter,
    Other(String),
}

/// One provider response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionResponse {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: FinishReason,
    pub usage: Usage,
}

impl CompletionResponse {
    /// Whether this response asks for tools to run.
    ///
    /// Reads the tool calls, not the finish reason: routers have been observed rewriting a
    /// `length` finish into `tool_calls` and vice versa, so the payload is the ground truth.
    pub fn requests_tools(&self) -> bool {
        !self.tool_calls.is_empty()
    }

    /// Whether the response carries neither text nor tool calls.
    ///
    /// Providers do emit these, and a loop that treats one as a final answer returns an
    /// empty reply to the user.
    pub fn is_empty(&self) -> bool {
        self.tool_calls.is_empty()
            && self
                .content
                .as_deref()
                .map(str::trim)
                .is_none_or(str::is_empty)
    }
}

/// A chat-completions-shaped model backend.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Identifier for logs and for telling two configured providers apart.
    fn name(&self) -> &str;

    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, LlmError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(
        content: Option<&str>,
        calls: Vec<ToolCall>,
        finish: FinishReason,
    ) -> CompletionResponse {
        CompletionResponse {
            content: content.map(str::to_owned),
            tool_calls: calls,
            finish_reason: finish,
            usage: Usage::default(),
        }
    }

    #[test]
    fn a_request_starts_without_tools_or_sampling_overrides() {
        let request =
            CompletionRequest::new("gpt-5", Arc::from("prompt"), vec![Message::user("hi")]);
        assert!(request.tools.is_empty());
        assert_eq!(request.temperature, None);
        assert_eq!(request.max_tokens, None);
    }

    #[test]
    fn builders_accumulate() {
        let request = CompletionRequest::new("gpt-5", Arc::from("prompt"), vec![])
            .with_tools(vec![ToolSpec {
                name: "now".into(),
                description: "d".into(),
                parameters: serde_json::json!({"type": "object"}),
            }])
            .with_temperature(0.2)
            .with_max_tokens(512);
        assert_eq!(request.tools.len(), 1);
        assert_eq!(request.temperature, Some(0.2));
        assert_eq!(request.max_tokens, Some(512));
    }

    #[test]
    fn tool_requests_are_detected_from_the_payload_not_the_finish_reason() {
        let calls = vec![ToolCall::new("c1", "now", "{}")];
        // A router rewrote the finish reason; the calls are still what matters.
        assert!(response(None, calls.clone(), FinishReason::Length).requests_tools());
        assert!(!response(Some("done"), vec![], FinishReason::ToolCalls).requests_tools());
    }

    #[test]
    fn empty_responses_are_recognised() {
        assert!(response(None, vec![], FinishReason::Stop).is_empty());
        assert!(response(Some(""), vec![], FinishReason::Stop).is_empty());
        assert!(response(Some("   \n"), vec![], FinishReason::Stop).is_empty());
    }

    #[test]
    fn a_response_with_content_or_calls_is_not_empty() {
        assert!(!response(Some("hi"), vec![], FinishReason::Stop).is_empty());
        assert!(
            !response(
                None,
                vec![ToolCall::new("c1", "now", "{}")],
                FinishReason::ToolCalls
            )
            .is_empty()
        );
    }

    #[test]
    fn finish_reasons_round_trip_through_serde() {
        for reason in [
            FinishReason::Stop,
            FinishReason::ToolCalls,
            FinishReason::Length,
            FinishReason::ContentFilter,
            FinishReason::Other("recitation".into()),
        ] {
            let json = serde_json::to_string(&reason).unwrap();
            assert_eq!(serde_json::from_str::<FinishReason>(&json).unwrap(), reason);
        }
        assert_eq!(
            serde_json::to_string(&FinishReason::ToolCalls).unwrap(),
            r#""tool_calls""#
        );
    }
}
