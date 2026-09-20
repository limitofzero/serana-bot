//! Error taxonomies shared across the domain.
//!
//! These are deliberately coarse. The turn loop branches on the *kind* of failure — retry,
//! compact the context, surface to the user, abort — so each variant exists because some
//! caller treats it differently. A variant nobody matches on is noise.

use thiserror::Error;

/// A failure returned by an [`crate::llm::LlmProvider`] implementation.
#[derive(Debug, Error)]
pub enum LlmError {
    /// The request never reached the provider, or the response never arrived.
    #[error("transport failure: {0}")]
    Transport(String),

    /// Provider-side rate limiting. `retry_after` is the provider's own hint, when it gave one.
    #[error("rate limited{}", .retry_after_secs.map(|s| format!(", retry after {s}s")).unwrap_or_default())]
    RateLimited { retry_after_secs: Option<u64> },

    /// The request exceeded the model's context window. The loop answers this by compacting,
    /// not by retrying, so it must stay distinguishable from a generic API error.
    #[error("context window exceeded")]
    ContextOverflow,

    /// Credentials missing, invalid, or lacking access to the requested model.
    #[error("authentication failed: {0}")]
    Auth(String),

    /// The provider answered with an error status we do not model more precisely.
    #[error("provider returned {status}: {message}")]
    Api { status: u16, message: String },

    /// The provider answered successfully but the body was not what the protocol promises.
    #[error("could not decode provider response: {0}")]
    Decode(String),
}

impl LlmError {
    /// Whether retrying the identical request could plausibly succeed.
    ///
    /// [`LlmError::ContextOverflow`] is deliberately *not* retryable: the same request will
    /// overflow again. It is resolved by compacting the conversation first.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Transport(_) | Self::RateLimited { .. } => true,
            Self::Api { status, .. } => *status >= 500,
            Self::ContextOverflow | Self::Auth(_) | Self::Decode(_) => false,
        }
    }
}

/// A failure inside a tool invocation.
///
/// Every variant is recoverable from the model's point of view: it is rendered into a
/// tool-role result and fed back, so the model can correct itself on the next iteration.
/// Tool failures never propagate as hard errors out of the turn loop.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ToolError {
    /// The arguments did not match the tool's schema, or were not valid JSON at all.
    #[error("invalid arguments: {0}")]
    InvalidArguments(String),

    /// The tool ran and failed.
    #[error("execution failed: {0}")]
    Execution(String),

    /// The tool cannot run in this session — missing credentials, disabled integration.
    #[error("unavailable: {0}")]
    Unavailable(String),
}

/// A failure from a persistence port ([`crate::memory::MemoryRepository`],
/// [`crate::conversation_store::ConversationRepository`]).
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("backend failure: {0}")]
    Backend(String),

    /// Stored bytes could not be read back into the domain type — a schema migration issue.
    #[error("could not deserialize stored record: {0}")]
    Corrupt(String),
}

/// A failure from [`crate::retrieval::Retriever`] or [`crate::retrieval::EmbeddingProvider`].
#[derive(Debug, Error)]
pub enum RetrievalError {
    #[error("embedding failed: {0}")]
    Embedding(String),

    #[error("index unavailable: {0}")]
    IndexUnavailable(String),

    #[error("backend failure: {0}")]
    Backend(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_and_rate_limit_are_retryable() {
        assert!(LlmError::Transport("reset".into()).is_retryable());
        assert!(
            LlmError::RateLimited {
                retry_after_secs: Some(3)
            }
            .is_retryable()
        );
    }

    #[test]
    fn server_errors_retry_but_client_errors_do_not() {
        assert!(
            LlmError::Api {
                status: 503,
                message: String::new()
            }
            .is_retryable()
        );
        assert!(
            !LlmError::Api {
                status: 400,
                message: String::new()
            }
            .is_retryable()
        );
    }

    #[test]
    fn overflow_is_not_retryable_because_it_is_resolved_by_compaction() {
        assert!(!LlmError::ContextOverflow.is_retryable());
    }

    #[test]
    fn auth_and_decode_are_terminal() {
        assert!(!LlmError::Auth("no key".into()).is_retryable());
        assert!(!LlmError::Decode("bad json".into()).is_retryable());
    }

    #[test]
    fn rate_limit_hint_reaches_the_message() {
        let with = LlmError::RateLimited {
            retry_after_secs: Some(12),
        }
        .to_string();
        let without = LlmError::RateLimited {
            retry_after_secs: None,
        }
        .to_string();
        assert!(with.contains("12s"), "{with}");
        assert_eq!(without, "rate limited");
    }
}
