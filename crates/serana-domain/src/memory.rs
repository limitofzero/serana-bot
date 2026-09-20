//! Long-term memory: what Serana knows between conversations.
//!
//! The article this project follows makes the point that memory is not a special mechanism
//! — it is a repository plus a few tools. RAG is the same repository with a different
//! implementation behind [`MemoryRepository::search`]: grep over Markdown files today, a
//! vector index later, with no change above this port.
//!
//! Retrieved memories reach the model as **tool results**, never by rewriting the system
//! prompt (see `docs/reference-notes.md` §2). That is what makes the swap invisible to
//! everything above: the retrieval strategy changes, the message shape does not.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::StorageError;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MemoryId(String);

impl MemoryId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for MemoryId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a memory is for.
///
/// The kind decides *when* a record is loaded, which is the whole point of separating them:
/// [`MemoryKind::Personality`] and [`MemoryKind::UserProfile`] are folded into the system
/// prompt once, at conversation construction, while the rest are fetched on demand through
/// a tool so they never inflate the prefix of every request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    /// How Serana talks. Self-authored; read at conversation start.
    Personality,
    /// What Serana knows about its user. Read at conversation start.
    UserProfile,
    /// A summary of one past conversation. Retrieved on demand.
    ConversationSummary,
    /// A standalone learned fact. Retrieved on demand.
    Fact,
}

impl MemoryKind {
    /// Whether records of this kind belong in the system prompt at conversation start.
    ///
    /// Everything else is retrieved mid-conversation, which — per the caching invariant —
    /// means it arrives as a tool result.
    pub fn is_preloaded(&self) -> bool {
        matches!(self, Self::Personality | Self::UserProfile)
    }
}

/// One stored memory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub id: MemoryId,
    pub kind: MemoryKind,
    pub title: String,
    pub body: String,
    pub created_at: jiff::Timestamp,
    pub updated_at: jiff::Timestamp,
}

/// A memory without its body, for listings.
///
/// Listing must not load bodies: a model that can see every title cheaply will ask for the
/// one it wants, whereas a listing that inlines bodies is just an expensive way to blow the
/// context window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemorySummary {
    pub id: MemoryId,
    pub kind: MemoryKind,
    pub title: String,
    pub updated_at: jiff::Timestamp,
}

impl From<&MemoryRecord> for MemorySummary {
    fn from(record: &MemoryRecord) -> Self {
        Self {
            id: record.id.clone(),
            kind: record.kind,
            title: record.title.clone(),
            updated_at: record.updated_at,
        }
    }
}

/// A search over stored memories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryQuery {
    pub text: String,
    /// Restrict to one kind, or search everything.
    pub kind: Option<MemoryKind>,
    pub limit: usize,
}

impl MemoryQuery {
    pub const DEFAULT_LIMIT: usize = 5;

    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            kind: None,
            limit: Self::DEFAULT_LIMIT,
        }
    }

    pub fn of_kind(mut self, kind: MemoryKind) -> Self {
        self.kind = Some(kind);
        self
    }

    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }
}

/// A search hit.
///
/// `score` is comparable only within one result set: a lexical backend and an embedding
/// backend do not produce scores on the same scale, and nothing above this port may assume
/// they do.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredMemory {
    pub record: MemoryRecord,
    pub score: f32,
}

/// Persistence for long-term memory.
///
/// `search` is the port's one interesting method and the seam RAG arrives through. A
/// filesystem implementation may answer it with a substring scan; a vector implementation
/// embeds the query and ranks by similarity. Callers only promise to treat the results as
/// ranked, best first.
#[async_trait]
pub trait MemoryRepository: Send + Sync {
    async fn get(&self, id: &MemoryId) -> Result<Option<MemoryRecord>, StorageError>;

    /// Insert or replace.
    async fn put(&self, record: MemoryRecord) -> Result<(), StorageError>;

    async fn delete(&self, id: &MemoryId) -> Result<(), StorageError>;

    /// Titles only, never bodies.
    async fn list(&self, kind: Option<MemoryKind>) -> Result<Vec<MemorySummary>, StorageError>;

    /// Ranked best first, at most `query.limit` results.
    async fn search(&self, query: &MemoryQuery) -> Result<Vec<ScoredMemory>, StorageError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(kind: MemoryKind) -> MemoryRecord {
        MemoryRecord {
            id: MemoryId::new("m1"),
            kind,
            title: "Restaurant choice".into(),
            body: "Decided on Ель, table at 21:00.".into(),
            created_at: jiff::Timestamp::UNIX_EPOCH,
            updated_at: jiff::Timestamp::UNIX_EPOCH,
        }
    }

    #[test]
    fn identity_memories_preload_and_the_rest_do_not() {
        assert!(MemoryKind::Personality.is_preloaded());
        assert!(MemoryKind::UserProfile.is_preloaded());
        assert!(!MemoryKind::ConversationSummary.is_preloaded());
        assert!(!MemoryKind::Fact.is_preloaded());
    }

    #[test]
    fn a_summary_drops_the_body() {
        let record = record(MemoryKind::ConversationSummary);
        let summary = MemorySummary::from(&record);
        assert_eq!(summary.id, record.id);
        assert_eq!(summary.title, record.title);
        assert_eq!(summary.kind, record.kind);
        let json = serde_json::to_string(&summary).unwrap();
        assert!(
            !json.contains("Ель"),
            "a summary must not carry the body: {json}"
        );
    }

    #[test]
    fn a_query_defaults_to_every_kind_and_a_bounded_limit() {
        let query = MemoryQuery::new("restaurant");
        assert_eq!(query.kind, None);
        assert_eq!(query.limit, MemoryQuery::DEFAULT_LIMIT);
    }

    #[test]
    fn query_builders_narrow_the_search() {
        let query = MemoryQuery::new("restaurant")
            .of_kind(MemoryKind::Fact)
            .limit(20);
        assert_eq!(query.kind, Some(MemoryKind::Fact));
        assert_eq!(query.limit, 20);
    }

    #[test]
    fn records_round_trip_through_serde() {
        let original = record(MemoryKind::Fact);
        let json = serde_json::to_string(&original).unwrap();
        assert_eq!(
            serde_json::from_str::<MemoryRecord>(&json).unwrap(),
            original
        );
        assert!(json.contains(r#""kind":"fact""#), "{json}");
    }
}
