//! A SQLite-backed [`ConversationRepository`].
//!
//! Its own pool over the same database file as the reminders. WAL lets the two coexist, and
//! sqlx's migration table makes running the same migration directory twice a no-op. One
//! shared pool would be tidier; two is simpler, and the write volume here is one row per
//! message the user sends.

use std::path::Path;

use async_trait::async_trait;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};

use serana_domain::StorageError;
use serana_domain::conversation::{Conversation, ConversationId};
use serana_domain::conversation_store::ConversationRepository;

/// Conversations in SQLite.
#[derive(Debug, Clone)]
pub struct SqliteConversationRepository {
    pool: SqlitePool,
}

impl SqliteConversationRepository {
    /// Open (creating if absent) the database at `path` and run migrations.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .foreign_keys(true);
        Self::from_options(options, SqlitePoolOptions::new().max_connections(5)).await
    }

    /// A private in-memory database. For tests.
    pub async fn in_memory() -> Result<Self, StorageError> {
        // Pinned to one connection that is never retired: a second connection to an
        // in-memory SQLite database is a second, empty database.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .idle_timeout(None)
            .max_lifetime(None);
        Self::from_options(SqliteConnectOptions::new().in_memory(true), pool).await
    }

    async fn from_options(
        options: SqliteConnectOptions,
        pool_options: SqlitePoolOptions,
    ) -> Result<Self, StorageError> {
        let pool = pool_options.connect_with(options).await.map_err(backend)?;
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .map_err(|e| StorageError::Backend(format!("migrations failed: {e}")))?;
        Ok(Self { pool })
    }
}

fn backend(error: impl std::fmt::Display) -> StorageError {
    StorageError::Backend(error.to_string())
}

fn to_nanos(at: jiff::Timestamp) -> Result<i64, StorageError> {
    i64::try_from(at.as_nanosecond())
        .map_err(|_| StorageError::Backend(format!("{at} does not fit in an i64 of nanoseconds")))
}

#[async_trait]
impl ConversationRepository for SqliteConversationRepository {
    async fn load(&self, id: &ConversationId) -> Result<Option<Conversation>, StorageError> {
        let row = sqlx::query("SELECT document FROM conversations WHERE id = ?")
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(backend)?;

        let Some(row) = row else { return Ok(None) };
        let document: String = row.try_get("document").map_err(backend)?;
        serde_json::from_str(&document)
            .map(Some)
            .map_err(|e| StorageError::Corrupt(format!("conversation {id} is not readable: {e}")))
    }

    async fn save(&self, conversation: &Conversation) -> Result<(), StorageError> {
        let document = serde_json::to_string(conversation)
            .map_err(|e| StorageError::Backend(format!("conversation is not serialisable: {e}")))?;

        sqlx::query(
            "INSERT INTO conversations (id, document, created_at, updated_at)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                 document = excluded.document,
                 updated_at = excluded.updated_at",
        )
        .bind(conversation.id().as_str())
        .bind(document)
        .bind(to_nanos(conversation.created_at())?)
        .bind(to_nanos(conversation.created_at())?)
        .execute(&self.pool)
        .await
        .map_err(backend)?;

        // `updated_at` is the moment of the write, not of the conversation's creation. Done
        // in SQL so it cannot drift from an injected clock the aggregate does not carry.
        sqlx::query(
            "UPDATE conversations
             SET updated_at = CAST(strftime('%s','now') AS INTEGER) * 1000000000
             WHERE id = ?",
        )
        .bind(conversation.id().as_str())
        .execute(&self.pool)
        .await
        .map_err(backend)?;

        Ok(())
    }

    async fn delete(&self, id: &ConversationId) -> Result<(), StorageError> {
        sqlx::query("DELETE FROM conversations WHERE id = ?")
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn list(&self) -> Result<Vec<ConversationId>, StorageError> {
        let rows = sqlx::query("SELECT id FROM conversations ORDER BY updated_at DESC")
            .fetch_all(&self.pool)
            .await
            .map_err(backend)?;
        rows.into_iter()
            .map(|row| {
                row.try_get::<String, _>("id")
                    .map(ConversationId::new)
                    .map_err(backend)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use serana_domain::message::ToolCall;

    use super::*;

    fn conversation(id: &str) -> Conversation {
        Conversation::new(
            ConversationId::new(id),
            "You manage a person's reminders.",
            jiff::Timestamp::UNIX_EPOCH,
        )
    }

    #[tokio::test]
    async fn a_conversation_round_trips_with_its_history_intact() {
        let repo = SqliteConversationRepository::in_memory().await.unwrap();
        let mut c = conversation("tg:42");
        c.push_user("remind me to call the bank").unwrap();
        c.push_assistant("What time?").unwrap();
        c.push_user("tomorrow at 9").unwrap();
        repo.save(&c).await.unwrap();

        let loaded = repo
            .load(&ConversationId::new("tg:42"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded.messages(), c.messages());
        assert_eq!(loaded.id(), c.id());
    }

    #[tokio::test]
    async fn the_system_prompt_comes_back_byte_for_byte() {
        // A repository that re-renders it on load breaks the provider's prompt cache just
        // as thoroughly as mutating it in memory would (docs/reference-notes.md §2).
        let repo = SqliteConversationRepository::in_memory().await.unwrap();
        let prompt = "Line one.\n  Indented two.\tTabbed.\nUnicode: каждый 20 число ✅\n";
        let c = Conversation::new(
            ConversationId::new("tg:1"),
            prompt,
            jiff::Timestamp::UNIX_EPOCH,
        );
        repo.save(&c).await.unwrap();

        let loaded = repo.load(c.id()).await.unwrap().unwrap();
        assert_eq!(loaded.system_prompt(), prompt);
        assert_eq!(loaded.system_prompt().len(), prompt.len());
    }

    #[tokio::test]
    async fn a_tool_round_survives_the_round_trip() {
        let repo = SqliteConversationRepository::in_memory().await.unwrap();
        let mut c = conversation("tg:42");
        c.push_user("remind me on the 20th").unwrap();
        let call = ToolCall::new("call_1", "create_reminder", r#"{"kind":"monthly"}"#);
        c.push_tool_calls(vec![call.clone()]).unwrap();
        c.push_tool_result(&call.id, r#"{"created":"r1"}"#).unwrap();
        repo.save(&c).await.unwrap();

        let loaded = repo.load(c.id()).await.unwrap().unwrap();
        assert_eq!(loaded.messages(), c.messages());
        assert!(
            loaded.pending_tool_calls().is_empty(),
            "the result came back with its call"
        );
    }

    #[tokio::test]
    async fn saving_twice_replaces_rather_than_duplicating() {
        let repo = SqliteConversationRepository::in_memory().await.unwrap();
        let mut c = conversation("tg:42");
        c.push_user("one").unwrap();
        repo.save(&c).await.unwrap();
        c.push_assistant("two").unwrap();
        c.push_user("three").unwrap();
        repo.save(&c).await.unwrap();

        assert_eq!(repo.list().await.unwrap().len(), 1);
        assert_eq!(repo.load(c.id()).await.unwrap().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn an_unknown_conversation_is_none_rather_than_an_error() {
        let repo = SqliteConversationRepository::in_memory().await.unwrap();
        assert!(
            repo.load(&ConversationId::new("nobody"))
                .await
                .unwrap()
                .is_none()
        );
        assert!(repo.list().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn deleting_removes_it_and_is_idempotent() {
        let repo = SqliteConversationRepository::in_memory().await.unwrap();
        let c = conversation("tg:42");
        repo.save(&c).await.unwrap();
        repo.delete(c.id()).await.unwrap();
        assert!(repo.load(c.id()).await.unwrap().is_none());
        // Deleting again is not an error: /compact on a fresh chat must not fail.
        repo.delete(c.id()).await.unwrap();
    }

    #[tokio::test]
    async fn an_unreadable_document_is_reported_as_corrupt_not_skipped() {
        let repo = SqliteConversationRepository::in_memory().await.unwrap();
        let c = conversation("tg:42");
        repo.save(&c).await.unwrap();
        sqlx::query("UPDATE conversations SET document = ? WHERE id = ?")
            .bind("{not json")
            .bind("tg:42")
            .execute(&repo.pool)
            .await
            .unwrap();

        assert!(matches!(
            repo.load(c.id()).await,
            Err(StorageError::Corrupt(_))
        ));
    }
}
