//! The reminder itself: what it says, when it next fires, and whether it is finished.

use serde::{Deserialize, Serialize};

use crate::error::StorageError;

use super::checklist::TodoItem;
use super::id::{ReminderId, TimeZoneName, UserId};
use super::recurrence::Recurrence;

/// A scheduled reminder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reminder {
    pub id: ReminderId,
    pub owner: UserId,
    /// What to send. Stored as the user's own words.
    pub text: String,
    /// The checklist, if there is one. Empty is the ordinary case: a reminder that is just
    /// a message.
    #[serde(default)]
    pub items: Vec<TodoItem>,
    pub recurrence: Recurrence,
    pub timezone: TimeZoneName,
    pub created_at: jiff::Timestamp,
    /// When this next fires. `None` means it never will again — a spent
    /// [`Recurrence::Once`], or a cancelled reminder.
    ///
    /// Denormalised onto the row so the scheduler's "what is due" query is one indexed
    /// lookup instead of evaluating every recurrence on every tick.
    pub next_fire_at: Option<jiff::Timestamp>,
    pub last_fired_at: Option<jiff::Timestamp>,
    /// The end of the period the user has already dealt with, if any.
    ///
    /// Occurrences at or before it are suppressed, which is what makes "done" mean "stop
    /// asking until next month" rather than "delete this". It needs no clearing: once the
    /// next period begins the watermark is in the past and stops having any effect.
    #[serde(default)]
    pub acknowledged_through: Option<jiff::Timestamp>,
}

impl Reminder {
    /// Recompute [`Reminder::next_fire_at`] from `now`.
    ///
    /// Called on creation and after every delivery. Returns the new value. An outstanding
    /// acknowledgement holds the search past the end of its period, so a delivery that
    /// races an acknowledgement cannot resurrect the occurrences it silenced.
    pub fn reschedule(
        &mut self,
        now: jiff::Timestamp,
    ) -> Result<Option<jiff::Timestamp>, StorageError> {
        let tz = self.timezone.resolve()?;
        let floor = self
            .acknowledged_through
            .map_or(now, |through| now.max(through));
        self.next_fire_at = self.recurrence.next_occurrence_after(floor, &tz);
        Ok(self.next_fire_at)
    }

    /// Record that the user has dealt with this period, and skip to the next one.
    ///
    /// Returns the new firing time: `None` means nothing further is scheduled, which for a
    /// [`Recurrence::Once`] is the normal outcome.
    pub fn acknowledge(
        &mut self,
        now: jiff::Timestamp,
    ) -> Result<Option<jiff::Timestamp>, StorageError> {
        let tz = self.timezone.resolve()?;
        self.acknowledged_through = self.recurrence.period_end_after(now, &tz);
        match self.acknowledged_through {
            Some(_) => self.reschedule(now),
            // A one-off has no next period: acknowledging it is the end of it.
            None => {
                self.next_fire_at = None;
                Ok(None)
            }
        }
    }

    /// Whether this reminder has a future firing time.
    pub fn is_active(&self) -> bool {
        self.next_fire_at.is_some()
    }

    /// Mark a delivery at `now` and compute the following occurrence.
    pub fn mark_fired(
        &mut self,
        now: jiff::Timestamp,
    ) -> Result<Option<jiff::Timestamp>, StorageError> {
        self.last_fired_at = Some(now);
        self.reschedule(now)
    }
}
