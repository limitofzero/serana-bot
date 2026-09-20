//! Persistence for conversations.

use async_trait::async_trait;

use crate::conversation::{Conversation, ConversationId};
use crate::error::StorageError;

/// Where conversations are kept between turns.
///
/// A conversation is saved after every turn, not only at the end: the Telegram frontend has
/// no "end", and a process restart mid-session must not lose the history. Implementations
/// must round-trip the system prompt byte for byte — a repository that re-renders it on
/// load breaks the caching invariant just as thoroughly as mutating it in memory would.
#[async_trait]
pub trait ConversationRepository: Send + Sync {
    async fn load(&self, id: &ConversationId) -> Result<Option<Conversation>, StorageError>;

    /// Insert or replace.
    async fn save(&self, conversation: &Conversation) -> Result<(), StorageError>;

    async fn delete(&self, id: &ConversationId) -> Result<(), StorageError>;

    /// Ids of every stored conversation, most recently created first.
    async fn list(&self) -> Result<Vec<ConversationId>, StorageError>;
}
