//! Storing reminders, and delivering them.

use std::sync::Arc;

use async_trait::async_trait;

use crate::error::{NotifyError, StorageError};

use super::entry::Reminder;
use super::id::{ReminderId, UserId};

/// Persistence for reminders.
#[async_trait]
pub trait ReminderRepository: Send + Sync {
    async fn get(&self, id: &ReminderId) -> Result<Option<Reminder>, StorageError>;

    /// Insert or replace.
    async fn put(&self, reminder: &Reminder) -> Result<(), StorageError>;

    async fn delete(&self, id: &ReminderId) -> Result<(), StorageError>;

    /// Every reminder owned by `owner`, soonest first, including spent ones.
    async fn list_for_owner(&self, owner: UserId) -> Result<Vec<Reminder>, StorageError>;

    /// Reminders whose `next_fire_at` is at or before `now`.
    ///
    /// The scheduler's hot path: implementations index `next_fire_at` rather than scanning.
    async fn due_at(&self, now: jiff::Timestamp) -> Result<Vec<Reminder>, StorageError>;
}

/// Delivers a message to a user out of band — the push half of a reminder.
#[async_trait]
pub trait Notifier: Send + Sync {
    /// Deliver `reminder` to `owner`.
    ///
    /// Takes the whole reminder rather than a rendered string because what arrives depends
    /// on it: a checklist shows what is still outstanding, a plain reminder shows its text.
    /// Composing that sentence is the frontend's job, and the frontend is the only layer
    /// that may read `serana-app`.
    /// `now` decides which checklist items count as outstanding. It is passed in rather
    /// than read from a clock here so that the whole tick — what is due, what is still
    /// undone, what is recorded — is reasoned about at one instant.
    async fn notify(
        &self,
        owner: UserId,
        reminder: &Reminder,
        now: jiff::Timestamp,
    ) -> Result<(), NotifyError>;
}

#[async_trait]
impl<T: ReminderRepository + ?Sized> ReminderRepository for Arc<T> {
    async fn get(&self, id: &ReminderId) -> Result<Option<Reminder>, StorageError> {
        (**self).get(id).await
    }

    async fn put(&self, reminder: &Reminder) -> Result<(), StorageError> {
        (**self).put(reminder).await
    }

    async fn delete(&self, id: &ReminderId) -> Result<(), StorageError> {
        (**self).delete(id).await
    }

    async fn list_for_owner(&self, owner: UserId) -> Result<Vec<Reminder>, StorageError> {
        (**self).list_for_owner(owner).await
    }

    async fn due_at(&self, now: jiff::Timestamp) -> Result<Vec<Reminder>, StorageError> {
        (**self).due_at(now).await
    }
}

#[async_trait]
impl<T: Notifier + ?Sized> Notifier for Arc<T> {
    async fn notify(
        &self,
        owner: UserId,
        reminder: &Reminder,
        now: jiff::Timestamp,
    ) -> Result<(), NotifyError> {
        (**self).notify(owner, reminder, now).await
    }
}
