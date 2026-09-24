//! An in-memory reminder repository.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use serana_domain::StorageError;
use serana_domain::reminder::{Reminder, ReminderId, ReminderRepository, UserId};

/// A [`ReminderRepository`] backed by a map.
///
/// Ordering guarantees match the port's contract — soonest first for a listing, due-first
/// for the scheduler — so a service that depends on them is not passing here and failing
/// against SQLite.
#[derive(Debug, Default)]
pub struct InMemoryReminderRepository {
    rows: Mutex<HashMap<ReminderId, Reminder>>,
}

impl InMemoryReminderRepository {
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed rows without going through the async port.
    pub fn seeded(reminders: impl IntoIterator<Item = Reminder>) -> Self {
        let repo = Self::new();
        {
            let mut rows = repo.rows.lock().expect("rows mutex poisoned");
            for reminder in reminders {
                rows.insert(reminder.id.clone(), reminder);
            }
        }
        repo
    }

    pub fn len(&self) -> usize {
        self.rows.lock().expect("rows mutex poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Sort by next firing time, soonest first; reminders that will never fire again go last.
fn by_next_fire(a: &Reminder, b: &Reminder) -> std::cmp::Ordering {
    match (a.next_fire_at, b.next_fire_at) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.created_at.cmp(&b.created_at),
    }
}

#[async_trait]
impl ReminderRepository for InMemoryReminderRepository {
    async fn get(&self, id: &ReminderId) -> Result<Option<Reminder>, StorageError> {
        Ok(self
            .rows
            .lock()
            .expect("rows mutex poisoned")
            .get(id)
            .cloned())
    }

    async fn put(&self, reminder: &Reminder) -> Result<(), StorageError> {
        self.rows
            .lock()
            .expect("rows mutex poisoned")
            .insert(reminder.id.clone(), reminder.clone());
        Ok(())
    }

    async fn delete(&self, id: &ReminderId) -> Result<(), StorageError> {
        self.rows
            .lock()
            .expect("rows mutex poisoned")
            .remove(id)
            .map(|_| ())
            .ok_or_else(|| StorageError::NotFound(id.to_string()))
    }

    async fn list_for_owner(&self, owner: UserId) -> Result<Vec<Reminder>, StorageError> {
        let mut found: Vec<Reminder> = self
            .rows
            .lock()
            .expect("rows mutex poisoned")
            .values()
            .filter(|r| r.owner == owner)
            .cloned()
            .collect();
        found.sort_by(by_next_fire);
        Ok(found)
    }

    async fn due_at(&self, now: jiff::Timestamp) -> Result<Vec<Reminder>, StorageError> {
        let mut due: Vec<Reminder> = self
            .rows
            .lock()
            .expect("rows mutex poisoned")
            .values()
            .filter(|r| r.next_fire_at.is_some_and(|at| at <= now))
            .cloned()
            .collect();
        due.sort_by(by_next_fire);
        Ok(due)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serana_domain::reminder::{Recurrence, TimeZoneName};

    fn reminder(id: &str, owner: i64, next_fire_at: Option<&str>) -> Reminder {
        Reminder {
            id: ReminderId::new(id),
            owner: UserId::new(owner),
            text: format!("reminder {id}"),
            items: Vec::new(),
            recurrence: Recurrence::Daily {
                at: jiff::civil::time(10, 0, 0, 0),
            },
            timezone: TimeZoneName::new("Asia/Tbilisi"),
            created_at: jiff::Timestamp::UNIX_EPOCH,
            next_fire_at: next_fire_at.map(|t| t.parse().expect("valid instant")),
            last_fired_at: None,
            acknowledged_through: None,
        }
    }

    #[tokio::test]
    async fn a_stored_reminder_comes_back() {
        let repo = InMemoryReminderRepository::new();
        assert!(repo.is_empty());
        let r = reminder("r1", 1, Some("2026-03-20T06:00:00Z"));
        repo.put(&r).await.unwrap();
        assert_eq!(repo.get(&ReminderId::new("r1")).await.unwrap(), Some(r));
    }

    #[tokio::test]
    async fn putting_the_same_id_replaces_it() {
        let repo = InMemoryReminderRepository::new();
        repo.put(&reminder("r1", 1, None)).await.unwrap();
        let mut updated = reminder("r1", 1, None);
        updated.text = "changed".into();
        repo.put(&updated).await.unwrap();
        assert_eq!(repo.len(), 1);
        assert_eq!(
            repo.get(&ReminderId::new("r1"))
                .await
                .unwrap()
                .unwrap()
                .text,
            "changed"
        );
    }

    #[tokio::test]
    async fn a_missing_reminder_is_none_and_deleting_it_is_an_error() {
        let repo = InMemoryReminderRepository::new();
        assert_eq!(repo.get(&ReminderId::new("nope")).await.unwrap(), None);
        assert!(matches!(
            repo.delete(&ReminderId::new("nope")).await,
            Err(StorageError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn a_listing_is_scoped_to_its_owner_and_sorted_soonest_first() {
        let repo = InMemoryReminderRepository::seeded([
            reminder("late", 1, Some("2026-03-20T06:00:00Z")),
            reminder("soon", 1, Some("2026-03-11T06:00:00Z")),
            reminder("spent", 1, None),
            reminder("other", 2, Some("2026-03-01T06:00:00Z")),
        ]);
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
    async fn due_at_returns_only_what_has_come_round_including_the_exact_instant() {
        let repo = InMemoryReminderRepository::seeded([
            reminder("past", 1, Some("2026-03-10T06:00:00Z")),
            reminder("exactly_now", 1, Some("2026-03-11T06:00:00Z")),
            reminder("future", 1, Some("2026-03-12T06:00:00Z")),
            reminder("spent", 1, None),
        ]);
        let now: jiff::Timestamp = "2026-03-11T06:00:00Z".parse().unwrap();
        let ids: Vec<String> = repo
            .due_at(now)
            .await
            .unwrap()
            .iter()
            .map(|r| r.id.to_string())
            .collect();
        assert_eq!(ids, vec!["past", "exactly_now"]);
    }

    #[tokio::test]
    async fn due_at_crosses_owners_because_the_scheduler_serves_everyone() {
        let repo = InMemoryReminderRepository::seeded([
            reminder("a", 1, Some("2026-03-10T06:00:00Z")),
            reminder("b", 2, Some("2026-03-10T06:00:00Z")),
        ]);
        let now: jiff::Timestamp = "2026-03-11T06:00:00Z".parse().unwrap();
        assert_eq!(repo.due_at(now).await.unwrap().len(), 2);
    }
}
