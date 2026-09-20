//! A provider that answers from a script.

use std::collections::VecDeque;
use std::sync::Mutex;

use async_trait::async_trait;
use serana_domain::LlmError;
use serana_domain::llm::{CompletionRequest, CompletionResponse, FinishReason, LlmProvider};
use serana_domain::message::{ToolCall, Usage};

/// An [`LlmProvider`] that returns queued responses in order and records what it was asked.
///
/// Queue the turns a test needs, run the service, then assert on both the outcome and the
/// requests — checking, for instance, that the system prompt was byte-identical across two
/// calls.
pub struct ScriptedLlm {
    queued: Mutex<VecDeque<Result<CompletionResponse, LlmError>>>,
    seen: Mutex<Vec<CompletionRequest>>,
}

impl Default for ScriptedLlm {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptedLlm {
    pub fn new() -> Self {
        Self {
            queued: Mutex::new(VecDeque::new()),
            seen: Mutex::new(Vec::new()),
        }
    }

    /// Queue a plain text answer.
    pub fn answering(self, content: &str) -> Self {
        self.push(Ok(CompletionResponse {
            content: Some(content.to_owned()),
            tool_calls: Vec::new(),
            finish_reason: FinishReason::Stop,
            usage: Usage::default(),
        }))
    }

    /// Queue a turn that asks for tools.
    pub fn calling(self, calls: Vec<ToolCall>) -> Self {
        self.push(Ok(CompletionResponse {
            content: None,
            tool_calls: calls,
            finish_reason: FinishReason::ToolCalls,
            usage: Usage::default(),
        }))
    }

    /// Queue a failure.
    pub fn failing(self, error: LlmError) -> Self {
        self.push(Err(error))
    }

    /// Queue an arbitrary response, for cases the helpers do not cover.
    pub fn responding(self, response: CompletionResponse) -> Self {
        self.push(Ok(response))
    }

    fn push(self, item: Result<CompletionResponse, LlmError>) -> Self {
        self.queued
            .lock()
            .expect("queue mutex poisoned")
            .push_back(item);
        self
    }

    /// Requests received so far, in order.
    pub fn requests(&self) -> Vec<CompletionRequest> {
        self.seen.lock().expect("seen mutex poisoned").clone()
    }

    pub fn call_count(&self) -> usize {
        self.seen.lock().expect("seen mutex poisoned").len()
    }

    /// How many scripted responses are still unused.
    ///
    /// Assert this is zero at the end of a test: leftovers mean the code under test took a
    /// different path than the script assumed, and the assertions that passed did so by
    /// accident.
    pub fn remaining(&self) -> usize {
        self.queued.lock().expect("queue mutex poisoned").len()
    }
}

#[async_trait]
impl LlmProvider for ScriptedLlm {
    fn name(&self) -> &str {
        "scripted"
    }

    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        self.seen.lock().expect("seen mutex poisoned").push(request);
        self.queued
            .lock()
            .expect("queue mutex poisoned")
            .pop_front()
            .unwrap_or_else(|| panic!("ScriptedLlm was called more times than it was scripted for"))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use serana_domain::message::Message;

    fn request(text: &str) -> CompletionRequest {
        CompletionRequest::new("m", Arc::from("prompt"), vec![Message::user(text)])
    }

    #[tokio::test]
    async fn scripted_responses_come_back_in_order() {
        let llm = ScriptedLlm::new().answering("first").answering("second");
        assert_eq!(llm.remaining(), 2);
        assert_eq!(
            llm.complete(request("a")).await.unwrap().content.unwrap(),
            "first"
        );
        assert_eq!(
            llm.complete(request("b")).await.unwrap().content.unwrap(),
            "second"
        );
        assert_eq!(llm.remaining(), 0);
    }

    #[tokio::test]
    async fn requests_are_recorded_for_inspection() {
        let llm = ScriptedLlm::new().answering("ok");
        llm.complete(request("hello")).await.unwrap();
        assert_eq!(llm.call_count(), 1);
        assert_eq!(llm.requests()[0].messages, vec![Message::user("hello")]);
    }

    #[tokio::test]
    async fn a_scripted_failure_surfaces_as_an_error() {
        let llm = ScriptedLlm::new().failing(LlmError::ContextOverflow);
        assert!(matches!(
            llm.complete(request("a")).await,
            Err(LlmError::ContextOverflow)
        ));
    }

    #[tokio::test]
    async fn tool_calls_can_be_scripted() {
        let llm = ScriptedLlm::new().calling(vec![ToolCall::new("c1", "now", "{}")]);
        let response = llm.complete(request("a")).await.unwrap();
        assert!(response.requests_tools());
        assert_eq!(response.tool_calls[0].name, "now");
    }

    #[tokio::test]
    #[should_panic(expected = "more times than it was scripted for")]
    async fn overrunning_the_script_fails_loudly_rather_than_inventing_an_answer() {
        let llm = ScriptedLlm::new().answering("only one");
        llm.complete(request("a")).await.unwrap();
        let _ = llm.complete(request("b")).await;
    }
}
