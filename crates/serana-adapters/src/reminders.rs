//! A SQLite-backed [`ReminderRepository`].
//!
//! Queries are written with the runtime API rather than sqlx's compile-time-checked
//! macros. The macros need a live database at build time, which would mean provisioning one
//! inside the Docker build and in CI to compile at all. The trade is real — column typos
//! become test failures instead of compile errors — so every query here is covered by a
//! test that round-trips through an actual database.

use std::path::Path;

use async_trait::async_trait;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};

use serana_domain::StorageError;
use serana_domain::reminder::{
    Recurrence, Reminder, ReminderId, ReminderRepository, TimeZoneName, UserId,
};

/// Reminders in SQLite.
#[derive(Debug, Clone)]
pub struct SqliteReminderRepository {
    pool: SqlitePool,
}

impl SqliteReminderRepository {
    /// Open (creating if absent) the database at `path` and run migrations.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            // WAL so the scheduler reading due rows does not block a command writing a new
            // reminder, and vice versa.
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .foreign_keys(true);
        Self::from_options(options, SqlitePoolOptions::new().max_connections(5)).await
    }

    /// A private in-memory database. For tests.
    pub async fn in_memory() -> Result<Self, StorageError> {
        // An in-memory SQLite database belongs to a single connection: a second connection
        // is a second, empty database. So the pool is pinned to one connection that is
        // never retired, or the schema would vanish between calls.
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

/// jiff nanoseconds are an `i128`; SQLite integers are 64-bit, which caps what we can store
/// at roughly 1678-2262. jiff itself reaches year 9999, so this conversion is a real
/// boundary, not a formality — a reminder past 2262 is refused rather than wrapped.
fn to_nanos(ts: jiff::Timestamp) -> Result<i64, StorageError> {
    i64::try_from(ts.as_nanosecond())
        .map_err(|_| StorageError::Corrupt(format!("timestamp {ts} is outside the storable range")))
}

/// Every `i64` is a valid instant for jiff, so this cannot fail in practice; the `Result`
/// is kept because nothing in sqlx's type mapping proves the column holds what we wrote.
fn from_nanos(nanos: i64) -> Result<jiff::Timestamp, StorageError> {
    jiff::Timestamp::from_nanosecond(i128::from(nanos))
        .map_err(|e| StorageError::Corrupt(format!("stored timestamp {nanos} is invalid: {e}")))
}

fn row_to_reminder(row: &sqlx::sqlite::SqliteRow) -> Result<Reminder, StorageError> {
    let recurrence: String = row.try_get("recurrence").map_err(backend)?;
    let next_fire_at: Option<i64> = row.try_get("next_fire_at").map_err(backend)?;
    let last_fired_at: Option<i64> = row.try_get("last_fired_at").map_err(backend)?;

    Ok(Reminder {
        id: ReminderId::new(row.try_get::<String, _>("id").map_err(backend)?),
        owner: UserId::new(row.try_get::<i64, _>("owner").map_err(backend)?),
        text: row.try_get("text").map_err(backend)?,
        recurrence: serde_json::from_str::<Recurrence>(&recurrence).map_err(|e| {
            StorageError::Corrupt(format!("recurrence {recurrence:?} is not readable: {e}"))
        })?,
        timezone: TimeZoneName::new(row.try_get::<String, _>("timezone").map_err(backend)?),
        created_at: from_nanos(row.try_get("created_at").map_err(backend)?)?,
        next_fire_at: next_fire_at.map(from_nanos).transpose()?,
        last_fired_at: last_fired_at.map(from_nanos).transpose()?,
    })
}

/// Soonest first, with rows that will never fire again last. Mirrors the ordering the
/// in-memory fake promises, so a service cannot pass against one and fail against the other.
///
/// A macro rather than a `const` because sqlx only accepts string literals, to keep dynamic
/// SQL from being built by accident.
macro_rules! order_by_next_fire {
    () => {
        " ORDER BY next_fire_at IS NULL, next_fire_at ASC, created_at ASC"
    };
}

#[async_trait]
impl ReminderRepository for SqliteReminderRepository {
    async fn get(&self, id: &ReminderId) -> Result<Option<Reminder>, StorageError> {
        let row = sqlx::query("SELECT * FROM reminders WHERE id = ?")
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(backend)?;
        row.as_ref().map(row_to_reminder).transpose()
    }

    async fn put(&self, reminder: &Reminder) -> Result<(), StorageError> {
        let recurrence = serde_json::to_string(&reminder.recurrence)
            .map_err(|e| StorageError::Backend(format!("recurrence is not serialisable: {e}")))?;

        sqlx::query(
            "INSERT INTO reminders
                 (id, owner, text, recurrence, timezone, created_at, next_fire_at, last_fired_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                 owner = excluded.owner,
                 text = excluded.text,
                 recurrence = excluded.recurrence,
                 timezone = excluded.timezone,
                 created_at = excluded.created_at,
                 next_fire_at = excluded.next_fire_at,
                 last_fired_at = excluded.last_fired_at",
        )
        .bind(reminder.id.as_str())
        .bind(reminder.owner.get())
        .bind(&reminder.text)
        .bind(recurrence)
        .bind(reminder.timezone.as_str())
        .bind(to_nanos(reminder.created_at)?)
        .bind(reminder.next_fire_at.map(to_nanos).transpose()?)
        .bind(reminder.last_fired_at.map(to_nanos).transpose()?)
        .execute(&self.pool)
        .await
        .map_err(backend)?;

        Ok(())
    }

    async fn delete(&self, id: &ReminderId) -> Result<(), StorageError> {
        let result = sqlx::query("DELETE FROM reminders WHERE id = ?")
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(backend)?;

        if result.rows_affected() == 0 {
            return Err(StorageError::NotFound(id.to_string()));
        }
        Ok(())
    }

    async fn list_for_owner(&self, owner: UserId) -> Result<Vec<Reminder>, StorageError> {
        let rows = sqlx::query(concat!(
            "SELECT * FROM reminders WHERE owner = ?",
            order_by_next_fire!()
        ))
        .bind(owner.get())
        .fetch_all(&self.pool)
        .await
        .map_err(backend)?;
        rows.iter().map(row_to_reminder).collect()
    }

    async fn due_at(&self, now: jiff::Timestamp) -> Result<Vec<Reminder>, StorageError> {
        let rows = sqlx::query(concat!(
            "SELECT * FROM reminders WHERE next_fire_at IS NOT NULL AND next_fire_at <= ?",
            order_by_next_fire!()
        ))
        .bind(to_nanos(now)?)
        .fetch_all(&self.pool)
        .await
        .map_err(backend)?;
        rows.iter().map(row_to_reminder).collect()
    }
}

#[cfg(test)]
mod tests {
    use serana_domain::reminder::Weekday;

    use super::*;

    fn ts(rfc3339: &str) -> jiff::Timestamp {
        rfc3339.parse().expect("valid instant")
    }

    fn reminder(id: &str, owner: i64, next_fire_at: Option<&str>) -> Reminder {
        Reminder {
            id: ReminderId::new(id),
            owner: UserId::new(owner),
            text: format!("reminder {id}"),
            recurrence: Recurrence::Monthly {
                day: 20,
                at: jiff::civil::time(10, 0, 0, 0),
            },
            timezone: TimeZoneName::new("Asia/Tbilisi"),
            created_at: ts("2026-03-01T00:00:00Z"),
            next_fire_at: next_fire_at.map(ts),
            last_fired_at: None,
        }
    }

    async fn repo() -> SqliteReminderRepository {
        SqliteReminderRepository::in_memory().await.unwrap()
    }

    async fn seeded(reminders: &[Reminder]) -> SqliteReminderRepository {
        let repo = repo().await;
        for reminder in reminders {
            repo.put(reminder).await.unwrap();
        }
        repo
    }

    #[tokio::test]
    async fn a_stored_reminder_comes_back_identical() {
        let repo = repo().await;
        let mut original = reminder("r1", 1, Some("2026-03-20T06:00:00Z"));
        original.last_fired_at = Some(ts("2026-02-20T06:00:00Z"));
        repo.put(&original).await.unwrap();
        assert_eq!(
            repo.get(&ReminderId::new("r1")).await.unwrap(),
            Some(original)
        );
    }

    #[tokio::test]
    async fn sub_second_precision_survives_the_round_trip() {
        // Instants are stored as nanoseconds precisely so this holds; milliseconds would
        // silently truncate and `put` then `get` would not be an identity.
        let repo = repo().await;
        let mut original = reminder("r1", 1, None);
        original.created_at = ts("2026-03-01T00:00:00.123456789Z");
        repo.put(&original).await.unwrap();
        let stored = repo.get(&ReminderId::new("r1")).await.unwrap().unwrap();
        assert_eq!(stored.created_at, original.created_at);
        assert_eq!(
            stored.created_at.to_string(),
            "2026-03-01T00:00:00.123456789Z"
        );
    }

    #[tokio::test]
    async fn every_recurrence_variant_round_trips() {
        let repo = repo().await;
        let variants = [
            Recurrence::Once {
                at: jiff::civil::date(2026, 3, 20).at(10, 0, 0, 0),
            },
            Recurrence::Daily {
                at: jiff::civil::time(10, 0, 0, 0),
            },
            Recurrence::Weekly {
                weekday: Weekday::Friday,
                at: jiff::civil::time(9, 30, 0, 0),
            },
            Recurrence::Monthly {
                day: 31,
                at: jiff::civil::time(10, 0, 0, 0),
            },
        ];
        for (index, recurrence) in variants.into_iter().enumerate() {
            let mut r = reminder(&format!("r{index}"), 1, None);
            r.recurrence = recurrence.clone();
            repo.put(&r).await.unwrap();
            let stored = repo.get(&r.id).await.unwrap().unwrap();
            assert_eq!(stored.recurrence, recurrence);
        }
    }

    #[tokio::test]
    async fn putting_the_same_id_replaces_it_rather_than_failing() {
        let repo = repo().await;
        repo.put(&reminder("r1", 1, None)).await.unwrap();
        let mut updated = reminder("r1", 1, Some("2026-04-20T06:00:00Z"));
        updated.text = "changed".into();
        repo.put(&updated).await.unwrap();

        let stored = repo.get(&ReminderId::new("r1")).await.unwrap().unwrap();
        assert_eq!(stored.text, "changed");
        assert_eq!(stored.next_fire_at, Some(ts("2026-04-20T06:00:00Z")));
        assert_eq!(repo.list_for_owner(UserId::new(1)).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_missing_reminder_is_none_and_deleting_it_is_not_found() {
        let repo = repo().await;
        assert_eq!(repo.get(&ReminderId::new("nope")).await.unwrap(), None);
        assert!(matches!(
            repo.delete(&ReminderId::new("nope")).await,
            Err(StorageError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn deleting_removes_exactly_one_row() {
        let repo = seeded(&[reminder("r1", 1, None), reminder("r2", 1, None)]).await;
        repo.delete(&ReminderId::new("r1")).await.unwrap();
        assert_eq!(repo.get(&ReminderId::new("r1")).await.unwrap(), None);
        assert!(repo.get(&ReminderId::new("r2")).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn a_listing_is_scoped_to_its_owner_and_sorted_soonest_first() {
        // Deliberately the same expectations as the in-memory fake's test: a service that
        // relies on this ordering must behave the same against either.
        let repo = seeded(&[
            reminder("late", 1, Some("2026-03-20T06:00:00Z")),
            reminder("soon", 1, Some("2026-03-11T06:00:00Z")),
            reminder("spent", 1, None),
            reminder("other", 2, Some("2026-03-01T06:00:00Z")),
        ])
        .await;

        let ids: Vec<String> = repo
            .list_for_owner(UserId::new(1))
            .await
            .unwrap()
            .iter()
            .map(|r| r.id.to_string())
            .collect();
        assert_eq!(
            ids,
            vec!["soon", "late", "spent"],
            "spent reminders sort last"
        );
    }

    #[tokio::test]
    async fn an_owner_with_nothing_gets_an_empty_list() {
        let repo = seeded(&[reminder("r1", 1, None)]).await;
        assert!(
            repo.list_for_owner(UserId::new(999))
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn due_at_includes_the_exact_instant_and_excludes_the_future() {
        let repo = seeded(&[
            reminder("past", 1, Some("2026-03-10T06:00:00Z")),
            reminder("exactly_now", 1, Some("2026-03-11T06:00:00Z")),
            reminder("future", 1, Some("2026-03-12T06:00:00Z")),
            reminder("spent", 1, None),
        ])
        .await;

        let ids: Vec<String> = repo
            .due_at(ts("2026-03-11T06:00:00Z"))
            .await
            .unwrap()
            .iter()
            .map(|r| r.id.to_string())
            .collect();
        assert_eq!(ids, vec!["past", "exactly_now"]);
    }

    #[tokio::test]
    async fn due_at_crosses_owners_because_the_scheduler_serves_everyone() {
        let repo = seeded(&[
            reminder("a", 1, Some("2026-03-10T06:00:00Z")),
            reminder("b", 2, Some("2026-03-10T06:00:00Z")),
        ])
        .await;
        assert_eq!(
            repo.due_at(ts("2026-03-11T06:00:00Z")).await.unwrap().len(),
            2
        );
    }

    #[tokio::test]
    async fn reminders_survive_reopening_the_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("serana.db");

        {
            let repo = SqliteReminderRepository::open(&path).await.unwrap();
            repo.put(&reminder("r1", 1, Some("2026-03-20T06:00:00Z")))
                .await
                .unwrap();
        }

        // Reopening also re-runs migrations, so this covers their idempotence.
        let reopened = SqliteReminderRepository::open(&path).await.unwrap();
        let stored = reopened.get(&ReminderId::new("r1")).await.unwrap().unwrap();
        assert_eq!(stored.text, "reminder r1");
        assert_eq!(stored.next_fire_at, Some(ts("2026-03-20T06:00:00Z")));
    }

    #[tokio::test]
    async fn an_unreadable_recurrence_is_reported_as_corrupt_not_skipped() {
        // A future variant, written by a newer build, read back by an older one.
        let repo = repo().await;
        repo.put(&reminder("r1", 1, None)).await.unwrap();
        sqlx::query("UPDATE reminders SET recurrence = ? WHERE id = ?")
            .bind(r#"{"kind":"every_fortnight"}"#)
            .bind("r1")
            .execute(&repo.pool)
            .await
            .unwrap();

        let err = repo.get(&ReminderId::new("r1")).await.unwrap_err();
        assert!(matches!(err, StorageError::Corrupt(_)), "{err:?}");
    }

    #[tokio::test]
    async fn the_whole_storable_range_reads_back_without_a_spurious_error() {
        // Every i64 is a valid instant for jiff, so reading must never invent a failure.
        let repo = repo().await;
        repo.put(&reminder("r1", 1, None)).await.unwrap();
        for nanos in [i64::MIN, 0, i64::MAX] {
            sqlx::query("UPDATE reminders SET created_at = ? WHERE id = ?")
                .bind(nanos)
                .bind("r1")
                .execute(&repo.pool)
                .await
                .unwrap();
            assert!(
                repo.get(&ReminderId::new("r1")).await.is_ok(),
                "{nanos} should be readable"
            );
        }
    }

    #[tokio::test]
    async fn an_instant_beyond_the_storable_range_is_refused_rather_than_wrapped() {
        // jiff reaches year 9999; nanoseconds in an i64 stop in 2262. Silently wrapping
        // would put a reminder somewhere in the seventeenth century.
        let repo = repo().await;
        let mut r = reminder("r1", 1, None);
        r.created_at = jiff::Timestamp::MAX;
        let err = repo.put(&r).await.unwrap_err();
        assert!(matches!(err, StorageError::Corrupt(_)), "{err:?}");
        assert_eq!(repo.get(&r.id).await.unwrap(), None, "nothing was written");
    }

    #[tokio::test]
    async fn text_is_stored_verbatim_including_non_ascii() {
        let repo = repo().await;
        let mut r = reminder("r1", 1, None);
        r.text = "оформить invoice 🧾".into();
        repo.put(&r).await.unwrap();
        assert_eq!(
            repo.get(&r.id).await.unwrap().unwrap().text,
            "оформить invoice 🧾"
        );
    }
}
