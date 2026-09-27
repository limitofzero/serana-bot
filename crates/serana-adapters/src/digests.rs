//! A SQLite-backed [`DigestRepository`].
//!
//! The digest is stored as one JSON document keyed by owner. Nothing in SQL reads inside it
//! — the watchlist, the schedule and the cached prices are always read and written together
//! — so a new field is a serde default rather than a migration. `next_fire_at` is lifted
//! out because the scheduler's hot path queries it on every tick.

use std::path::Path;

use async_trait::async_trait;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};

use serana_domain::StorageError;
use serana_domain::prices::{Digest, DigestRepository};
use serana_domain::reminder::UserId;

use crate::sqlite::{backend, open_pool, to_nanos};

/// Digests in SQLite.
#[derive(Debug, Clone)]
pub struct SqliteDigestRepository {
    pool: SqlitePool,
}

impl SqliteDigestRepository {
    /// Open (creating if absent) the database at `path` and run migrations.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        Ok(Self {
            pool: open_pool(
                SqliteConnectOptions::new()
                    .filename(path)
                    .create_if_missing(true)
                    .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
                    .foreign_keys(true),
                SqlitePoolOptions::new().max_connections(5),
            )
            .await?,
        })
    }

    /// A private in-memory database. For tests.
    pub async fn in_memory() -> Result<Self, StorageError> {
        Ok(Self {
            pool: open_pool(
                SqliteConnectOptions::new().in_memory(true),
                SqlitePoolOptions::new()
                    .max_connections(1)
                    .idle_timeout(None)
                    .max_lifetime(None),
            )
            .await?,
        })
    }
}

fn row_to_digest(row: &sqlx::sqlite::SqliteRow) -> Result<Digest, StorageError> {
    let document: String = row.try_get("document").map_err(backend)?;
    serde_json::from_str(&document)
        .map_err(|e| StorageError::Corrupt(format!("stored digest is not readable: {e}")))
}

#[async_trait]
impl DigestRepository for SqliteDigestRepository {
    async fn get(&self, owner: UserId) -> Result<Option<Digest>, StorageError> {
        let row = sqlx::query("SELECT document FROM digests WHERE owner = ?")
            .bind(owner.get())
            .fetch_optional(&self.pool)
            .await
            .map_err(backend)?;
        row.as_ref().map(row_to_digest).transpose()
    }

    async fn put(&self, digest: &Digest) -> Result<(), StorageError> {
        let next_fire_at = digest.next_fire_at.map(to_nanos).transpose()?;
        let document = serde_json::to_string(digest)
            .map_err(|e| StorageError::Corrupt(format!("digest is not storable: {e}")))?;

        sqlx::query(
            "INSERT INTO digests (owner, next_fire_at, document) VALUES (?, ?, ?) \
             ON CONFLICT(owner) DO UPDATE SET next_fire_at = excluded.next_fire_at, \
             document = excluded.document",
        )
        .bind(digest.owner.get())
        .bind(next_fire_at)
        .bind(document)
        .execute(&self.pool)
        .await
        .map_err(backend)?;
        Ok(())
    }

    async fn due_at(&self, now: jiff::Timestamp) -> Result<Vec<Digest>, StorageError> {
        let rows = sqlx::query(
            "SELECT document FROM digests \
             WHERE next_fire_at IS NOT NULL AND next_fire_at <= ? ORDER BY next_fire_at",
        )
        .bind(to_nanos(now)?)
        .fetch_all(&self.pool)
        .await
        .map_err(backend)?;
        rows.iter().map(row_to_digest).collect()
    }
}

#[cfg(test)]
mod tests;
