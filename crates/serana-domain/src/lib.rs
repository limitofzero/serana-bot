//! Serana's domain: the types the whole system speaks in, and the port traits every
//! adapter implements.
//!
//! This crate is pure. It performs no I/O, opens no sockets, reads no clock and touches no
//! filesystem — those are ports here and implementations in `serana-adapters`. The
//! dependency rule is enforced by cargo: nothing in this crate may depend on the services
//! or adapters crates, so a type that needs a runtime does not belong here.
//!
//! Two invariants are load-bearing and encoded in the types rather than left to review:
//! the system prompt is byte-stable for the life of a conversation, and messages alternate
//! strictly. Both live in [`conversation`]; `docs/reference-notes.md` records why.

pub mod clock;
pub mod conversation;
pub mod conversation_store;
pub mod error;
pub mod ids;
pub mod llm;
pub mod memory;
pub mod message;
pub mod reminder;
pub mod retrieval;
pub mod tool;

pub use clock::Clock;
pub use conversation::{AlternationError, Conversation, ConversationId, Tail};
pub use conversation_store::ConversationRepository;
pub use error::{LlmError, RetrievalError, StorageError, ToolError};
pub use ids::IdGenerator;
pub use llm::{CompletionRequest, CompletionResponse, FinishReason, LlmProvider};
pub use memory::{
    MemoryId, MemoryKind, MemoryQuery, MemoryRecord, MemoryRepository, MemorySummary, ScoredMemory,
};
pub use message::{Message, ToolCall, ToolCallId, Usage};
pub use reminder::{
    Notifier, NotifyError, Recurrence, Reminder, ReminderId, ReminderRepository, TimeZoneName,
    UserId, Weekday,
};
pub use retrieval::{Embedding, EmbeddingProvider, Passage, Retriever};
pub use tool::{Tool, ToolRegistry, ToolSpec};
