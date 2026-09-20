//! An [`LlmProvider`] over any OpenAI-compatible `chat/completions` endpoint.

mod wire;

use std::time::Duration;

use async_trait::async_trait;
use backon::{ExponentialBuilder, Retryable};
use reqwest::StatusCode;

use serana_domain::LlmError;
use serana_domain::llm::{CompletionRequest, CompletionResponse, LlmProvider};

use wire::{ChatRequest, ChatResponse, WireErrorEnvelope};

/// How to reach the provider.
#[derive(Debug, Clone)]
pub struct OpenAiConfig {
    /// Endpoint root, with or without a trailing slash — `https://api.openai.com/v1`.
    pub base_url: String,
    pub api_key: String,
    pub request_timeout: Duration,
    /// Retries *after* the first attempt, for transport failures, 429s and 5xxs only.
    pub max_retries: usize,
    /// First backoff delay; subsequent ones grow exponentially. Tests set this tiny.
    pub retry_min_delay: Duration,
}

impl Default for OpenAiConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com/v1".to_owned(),
            api_key: String::new(),
            request_timeout: Duration::from_secs(120),
            max_retries: 3,
            retry_min_delay: Duration::from_millis(500),
        }
    }
}

impl OpenAiConfig {
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: api_key.into(),
            ..Self::default()
        }
    }
}

/// Talks chat-completions to one endpoint.
///
/// Retries live here, but only the transport-shaped ones — a connection reset, a 429, a
/// 502. Semantic recovery (compacting on overflow, repairing a hallucinated tool name) is
/// the turn loop's job, because it needs to change the request to have any chance of
/// succeeding, and this layer is not allowed to do that.
pub struct OpenAiProvider {
    http: reqwest::Client,
    endpoint: String,
    api_key: String,
    max_retries: usize,
    retry_min_delay: Duration,
}

impl OpenAiProvider {
    pub fn new(config: OpenAiConfig) -> Result<Self, LlmError> {
        let http = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .build()
            .map_err(|e| LlmError::Transport(format!("could not build HTTP client: {e}")))?;

        Ok(Self {
            http,
            endpoint: format!("{}/chat/completions", config.base_url.trim_end_matches('/')),
            api_key: config.api_key,
            max_retries: config.max_retries,
            retry_min_delay: config.retry_min_delay,
        })
    }

    async fn attempt(&self, body: &ChatRequest) -> Result<ChatResponse, LlmError> {
        let response = self
            .http
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(body)
            .send()
            .await
            .map_err(|e| LlmError::Transport(e.to_string()))?;

        let status = response.status();
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(classify(status, retry_after, &body));
        }

        response
            .json::<ChatResponse>()
            .await
            .map_err(|e| LlmError::Decode(e.to_string()))
    }
}

/// Map an HTTP failure onto the taxonomy the turn loop branches on.
fn classify(status: StatusCode, retry_after: Option<u64>, body: &str) -> LlmError {
    let envelope: WireErrorEnvelope = serde_json::from_str(body).unwrap_or_default();
    let error = envelope.error.unwrap_or_default();
    let message = if error.message.is_empty() {
        body.to_owned()
    } else {
        error.message
    };

    // Overflow is a 400 like any other, so it has to be recognised from the body. Getting
    // this wrong means the loop retries an identical request that cannot ever fit.
    let looks_like_overflow = {
        let haystack = format!(
            "{} {} {}",
            message.to_lowercase(),
            error.code.unwrap_or_default().to_lowercase(),
            error.kind.unwrap_or_default().to_lowercase()
        );
        [
            "context_length_exceeded",
            "context length",
            "maximum context",
            "too many tokens",
        ]
        .iter()
        .any(|needle| haystack.contains(needle))
    };

    match status {
        _ if looks_like_overflow => LlmError::ContextOverflow,
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => LlmError::Auth(message),
        StatusCode::TOO_MANY_REQUESTS => LlmError::RateLimited {
            retry_after_secs: retry_after,
        },
        StatusCode::PAYLOAD_TOO_LARGE => LlmError::ContextOverflow,
        _ => LlmError::Api {
            status: status.as_u16(),
            message,
        },
    }
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    fn name(&self) -> &str {
        "openai"
    }

    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let body = ChatRequest::from(&request);

        let response = (|| async { self.attempt(&body).await })
            .retry(
                ExponentialBuilder::default()
                    .with_min_delay(self.retry_min_delay)
                    .with_max_times(self.max_retries),
            )
            .when(LlmError::is_retryable)
            .await?;

        let choice = response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| LlmError::Decode("response contained no choices".into()))?;

        Ok(CompletionResponse {
            content: choice.message.content,
            tool_calls: choice
                .message
                .tool_calls
                .unwrap_or_default()
                .into_iter()
                .map(Into::into)
                .collect(),
            finish_reason: wire::finish_reason(choice.finish_reason),
            usage: response.usage.unwrap_or_default().into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serana_domain::llm::FinishReason;
    use serana_domain::message::{Message, ToolCall};
    use serana_domain::tool::ToolSpec;
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    /// A provider pointed at `server`, with retries fast enough for a test suite.
    fn provider(server: &MockServer, max_retries: usize) -> OpenAiProvider {
        OpenAiProvider::new(OpenAiConfig {
            base_url: server.uri(),
            api_key: "test-key".into(),
            request_timeout: Duration::from_secs(5),
            max_retries,
            retry_min_delay: Duration::from_millis(1),
        })
        .unwrap()
    }

    fn request() -> CompletionRequest {
        CompletionRequest::new(
            "gpt-5",
            Arc::from("You are Serana."),
            vec![Message::user("what day is it?")],
        )
    }

    fn answer(content: &str) -> serde_json::Value {
        json!({
            "choices": [{"message": {"role": "assistant", "content": content},
                         "finish_reason": "stop"}]
        })
    }

    async fn serving(body: serde_json::Value, status: u16) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .mount(&server)
            .await;
        server
    }

    /// The body of the single request the server received.
    async fn sent_body(server: &MockServer) -> serde_json::Value {
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1, "expected exactly one request");
        serde_json::from_slice(&requests[0].body).unwrap()
    }

    #[tokio::test]
    async fn the_system_prompt_is_sent_first_and_verbatim() {
        // The invariant the domain protects is only worth anything if the adapter honours
        // it at the boundary.
        let server = serving(answer("Friday"), 200).await;
        provider(&server, 0).complete(request()).await.unwrap();

        let body = sent_body(&server).await;
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][0]["content"], "You are Serana.");
        assert_eq!(body["messages"][1]["role"], "user");
        assert_eq!(body["messages"][1]["content"], "what day is it?");
        assert_eq!(body["model"], "gpt-5");
    }

    #[tokio::test]
    async fn the_api_key_is_sent_as_a_bearer_token() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(header("authorization", "Bearer test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(answer("ok")))
            .expect(1)
            .mount(&server)
            .await;
        provider(&server, 0).complete(request()).await.unwrap();
    }

    #[tokio::test]
    async fn a_base_url_with_a_trailing_slash_does_not_double_it() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(answer("ok")))
            .expect(1)
            .mount(&server)
            .await;
        let provider = OpenAiProvider::new(OpenAiConfig {
            base_url: format!("{}/", server.uri()),
            api_key: "k".into(),
            retry_min_delay: Duration::from_millis(1),
            ..OpenAiConfig::default()
        })
        .unwrap();
        provider.complete(request()).await.unwrap();
    }

    #[tokio::test]
    async fn a_plain_answer_parses() {
        let server = serving(answer("It is Friday."), 200).await;
        let response = provider(&server, 0).complete(request()).await.unwrap();
        assert_eq!(response.content.as_deref(), Some("It is Friday."));
        assert_eq!(response.finish_reason, FinishReason::Stop);
        assert!(!response.requests_tools());
    }

    #[tokio::test]
    async fn tool_call_arguments_survive_as_raw_text() {
        // Not parsed here on purpose: malformed argument JSON has to reach the validator,
        // which can report it back to the model, rather than dying as a decode error.
        let server = serving(
            json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "call_1",
                            "type": "function",
                            "function": {"name": "get_weekday", "arguments": "{\"tz\":\"Asia/Tbilisi\""}
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            }),
            200,
        )
        .await;

        let response = provider(&server, 0).complete(request()).await.unwrap();
        assert!(response.requests_tools());
        let call = &response.tool_calls[0];
        assert_eq!(call.id.as_str(), "call_1");
        assert_eq!(call.name, "get_weekday");
        assert_eq!(call.arguments, "{\"tz\":\"Asia/Tbilisi\"");
        assert!(
            call.parse_arguments().is_err(),
            "the truncation is preserved, not hidden"
        );
    }

    #[tokio::test]
    async fn an_explicit_null_tool_calls_field_is_not_an_error() {
        let server = serving(
            json!({"choices": [{"message": {"role": "assistant", "content": "hi",
                                            "tool_calls": null}, "finish_reason": "stop"}]}),
            200,
        )
        .await;
        let response = provider(&server, 0).complete(request()).await.unwrap();
        assert!(response.tool_calls.is_empty());
    }

    #[tokio::test]
    async fn a_missing_finish_reason_is_treated_as_a_normal_stop() {
        let server = serving(
            json!({"choices": [{"message": {"role": "assistant", "content": "hi"}}]}),
            200,
        )
        .await;
        let response = provider(&server, 0).complete(request()).await.unwrap();
        assert_eq!(response.finish_reason, FinishReason::Stop);
    }

    #[tokio::test]
    async fn an_unrecognised_finish_reason_is_kept_rather_than_discarded() {
        let server = serving(
            json!({"choices": [{"message": {"role": "assistant", "content": "hi"},
                                "finish_reason": "recitation"}]}),
            200,
        )
        .await;
        let response = provider(&server, 0).complete(request()).await.unwrap();
        assert_eq!(
            response.finish_reason,
            FinishReason::Other("recitation".into())
        );
    }

    #[tokio::test]
    async fn cached_prompt_tokens_are_read_from_the_usage_details() {
        // Without this the prompt-cache invariant cannot be observed in production.
        let server = serving(
            json!({
                "choices": [{"message": {"role": "assistant", "content": "hi"},
                             "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 1200, "completion_tokens": 40,
                          "prompt_tokens_details": {"cached_tokens": 1024}}
            }),
            200,
        )
        .await;
        let usage = provider(&server, 0)
            .complete(request())
            .await
            .unwrap()
            .usage;
        assert_eq!(usage.prompt_tokens, 1200);
        assert_eq!(usage.completion_tokens, 40);
        assert_eq!(usage.cached_prompt_tokens, 1024);
        assert_eq!(usage.total(), 1240);
    }

    #[tokio::test]
    async fn a_response_without_usage_reports_zeroes_rather_than_failing() {
        let server = serving(answer("hi"), 200).await;
        let usage = provider(&server, 0)
            .complete(request())
            .await
            .unwrap()
            .usage;
        assert_eq!(usage.total(), 0);
    }

    #[tokio::test]
    async fn tools_are_sent_in_the_function_envelope() {
        let server = serving(answer("ok"), 200).await;
        let spec = ToolSpec {
            name: "get_weekday".into(),
            description: "Return today's weekday.".into(),
            parameters: json!({"type": "object", "properties": {}}),
        };
        provider(&server, 0)
            .complete(request().with_tools(vec![spec]))
            .await
            .unwrap();

        let body = sent_body(&server).await;
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], "get_weekday");
        assert_eq!(body["tools"][0]["function"]["parameters"]["type"], "object");
    }

    #[tokio::test]
    async fn a_request_without_tools_omits_the_field_entirely() {
        // Sending `"tools": []` makes some gateways reject the request outright.
        let server = serving(answer("ok"), 200).await;
        provider(&server, 0).complete(request()).await.unwrap();
        let body = sent_body(&server).await;
        assert!(body.get("tools").is_none(), "{body}");
        assert!(body.get("temperature").is_none());
        assert!(body.get("max_tokens").is_none());
    }

    #[tokio::test]
    async fn a_full_tool_round_trips_back_onto_the_wire() {
        let server = serving(answer("It is Friday."), 200).await;
        let call = ToolCall::new("call_1", "get_weekday", "{}");
        let messages = vec![
            Message::user("what day is it?"),
            Message::assistant_tool_calls(vec![call.clone()]),
            Message::tool_result(&call, "Friday"),
        ];
        provider(&server, 0)
            .complete(CompletionRequest::new(
                "gpt-5",
                Arc::from("prompt"),
                messages,
            ))
            .await
            .unwrap();

        let body = sent_body(&server).await;
        assert_eq!(body["messages"][2]["role"], "assistant");
        assert_eq!(body["messages"][2]["tool_calls"][0]["id"], "call_1");
        assert_eq!(body["messages"][3]["role"], "tool");
        assert_eq!(body["messages"][3]["tool_call_id"], "call_1");
        assert_eq!(body["messages"][3]["content"], "Friday");
    }

    #[tokio::test]
    async fn an_unauthorised_response_is_an_auth_error_and_is_not_retried() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(401)
                    .set_body_json(json!({"error": {"message": "Incorrect API key provided"}})),
            )
            .expect(1)
            .mount(&server)
            .await;

        let err = provider(&server, 3).complete(request()).await.unwrap_err();
        assert!(matches!(err, LlmError::Auth(m) if m.contains("Incorrect API key")),);
    }

    #[tokio::test]
    async fn a_rate_limit_carries_the_providers_retry_hint() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("retry-after", "42")
                    .set_body_json(json!({"error": {"message": "slow down"}})),
            )
            .mount(&server)
            .await;

        let err = provider(&server, 0).complete(request()).await.unwrap_err();
        assert!(
            matches!(
                err,
                LlmError::RateLimited {
                    retry_after_secs: Some(42)
                }
            ),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn a_context_overflow_is_recognised_from_the_body_not_the_status() {
        // It arrives as a plain 400; misclassifying it makes the loop retry a request that
        // can never fit.
        let server = serving(
            json!({"error": {"message": "This model's maximum context length is 128000 tokens",
                             "code": "context_length_exceeded", "type": "invalid_request_error"}}),
            400,
        )
        .await;
        let err = provider(&server, 0).complete(request()).await.unwrap_err();
        assert!(matches!(err, LlmError::ContextOverflow), "{err:?}");
        assert!(!err.is_retryable());
    }

    #[tokio::test]
    async fn an_ordinary_bad_request_stays_an_api_error() {
        let server = serving(
            json!({"error": {"message": "unknown parameter 'foo'"}}),
            400,
        )
        .await;
        let err = provider(&server, 0).complete(request()).await.unwrap_err();
        assert!(matches!(err, LlmError::Api { status: 400, .. }), "{err:?}");
    }

    #[tokio::test]
    async fn an_error_body_that_is_not_json_still_reaches_the_message() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(502).set_body_string("<html>bad gateway</html>"))
            .mount(&server)
            .await;
        let err = provider(&server, 0).complete(request()).await.unwrap_err();
        assert!(
            matches!(&err, LlmError::Api { status: 502, message } if message.contains("bad gateway")),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn a_transient_server_error_is_retried_and_then_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(2)
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(answer("recovered")))
            .expect(1)
            .mount(&server)
            .await;

        let response = provider(&server, 3).complete(request()).await.unwrap();
        assert_eq!(response.content.as_deref(), Some("recovered"));
    }

    #[tokio::test]
    async fn retries_are_bounded() {
        let server = MockServer::start().await;
        // One initial attempt plus two retries.
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503))
            .expect(3)
            .mount(&server)
            .await;

        let err = provider(&server, 2).complete(request()).await.unwrap_err();
        assert!(matches!(err, LlmError::Api { status: 503, .. }), "{err:?}");
    }

    #[tokio::test]
    async fn a_malformed_success_body_is_a_decode_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json at all"))
            .expect(1)
            .mount(&server)
            .await;
        let err = provider(&server, 2).complete(request()).await.unwrap_err();
        assert!(matches!(err, LlmError::Decode(_)), "{err:?}");
    }

    #[tokio::test]
    async fn a_success_with_no_choices_is_a_decode_error_rather_than_an_empty_answer() {
        let server = serving(json!({"choices": []}), 200).await;
        let err = provider(&server, 0).complete(request()).await.unwrap_err();
        assert!(matches!(err, LlmError::Decode(m) if m.contains("no choices")));
    }

    #[tokio::test]
    async fn an_unreachable_endpoint_is_a_transport_error() {
        let provider = OpenAiProvider::new(OpenAiConfig {
            // Reserved for documentation, so nothing listens here.
            base_url: "http://192.0.2.1:9".into(),
            api_key: "k".into(),
            request_timeout: Duration::from_millis(150),
            max_retries: 0,
            retry_min_delay: Duration::from_millis(1),
        })
        .unwrap();
        let err = provider.complete(request()).await.unwrap_err();
        assert!(matches!(err, LlmError::Transport(_)), "{err:?}");
    }
}
