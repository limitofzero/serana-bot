//! Reading and changing the person's calendar.
//!
//! **Everything an event carries is untrusted.** Titles, locations and descriptions are
//! written by whoever sent the invitation — not by the person using this assistant. They
//! reach the model as tool results, which is data, and must never be read as instructions.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// A provider's identifier for an event. Opaque; only meaningful to the provider.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EventId(String);

impl EventId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for EventId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One entry on a calendar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarEvent {
    pub id: EventId,
    /// The title, as whoever created the event wrote it. Untrusted.
    pub summary: String,
    pub starts_at: jiff::Timestamp,
    pub ends_at: jiff::Timestamp,
    /// All-day events have no meaningful time of day, and saying "09:00" for one would be
    /// an invention.
    pub all_day: bool,
    /// Where it happens, as written by whoever created it. Untrusted.
    pub location: Option<String>,
    /// Whether the person owns this event.
    ///
    /// This decides what "cancel" can mean. Their own event can have an occurrence struck
    /// off; somebody else's can only be declined, which leaves it on the calendar marked as
    /// not attending. Getting this backwards either silently fails or deletes something
    /// that was not ours to delete.
    pub mine: bool,
    /// Whether this is one occurrence of a repeating event.
    ///
    /// Cancelling one occurrence of a series is a different act from cancelling the series,
    /// and the person almost always means the first. Knowing which this is lets the
    /// assistant say which it did.
    pub recurring: bool,
}

impl CalendarEvent {
    /// Whether this event overlaps the span `from`..`to`. Half-open, so two back-to-back
    /// meetings never both claim the same instant.
    pub fn overlaps(&self, from: jiff::Timestamp, to: jiff::Timestamp) -> bool {
        self.starts_at < to && from < self.ends_at
    }

    /// How "cancel this" has to be carried out for this event.
    pub fn cancellation(&self) -> Cancellation {
        if self.mine {
            Cancellation::StrikeOff
        } else {
            Cancellation::Decline
        }
    }
}

/// What cancelling an event actually does to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cancellation {
    /// Ours: the occurrence is struck off and disappears from that day. A series carries on.
    StrikeOff,
    /// Somebody else's: we decline. It stays on the calendar, marked as not attending,
    /// because removing another person's event from their own invitation is not ours to do.
    Decline,
}

/// An event to be created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewEvent {
    /// The title. Written by the model from what the person said.
    pub summary: String,
    /// When the appointment itself begins and ends — not the block that will be reserved.
    pub starts_at: jiff::Timestamp,
    pub ends_at: jiff::Timestamp,
    /// How long to leave either side for getting there and back.
    ///
    /// Zero for anything online. The block put on the calendar is the appointment plus this
    /// at both ends, so "am I free at 14:45?" answers correctly while the person is on the
    /// road — which is the entire reason it is not simply noted in the description.
    pub travel: std::time::Duration,
    pub location: Option<String>,
}

impl NewEvent {
    /// The span to reserve: the appointment, plus travel at each end.
    pub fn blocked(&self) -> (jiff::Timestamp, jiff::Timestamp) {
        let Ok(travel) = jiff::Span::try_from(self.travel) else {
            // An unrepresentable span means a nonsense argument; reserving the appointment
            // alone is the safe reading of it.
            return (self.starts_at, self.ends_at);
        };
        let from = self.starts_at.checked_sub(travel).unwrap_or(self.starts_at);
        let to = self.ends_at.checked_add(travel).unwrap_or(self.ends_at);
        (from, to)
    }

    /// Whether any travel is being reserved around the appointment.
    pub fn has_travel(&self) -> bool {
        !self.travel.is_zero()
    }
}

/// Why a calendar operation could not be carried out.
///
/// Two variants, because callers only behave two ways: there is no calendar connected, or
/// there is one and it did not answer. Splitting the second into auth, transport and decode
/// would produce three arms that all say "the calendar is unavailable".
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CalendarError {
    /// No calendar is configured. Not an outage: the assistant simply has none, and should
    /// say so rather than apologising for a failure.
    #[error("no calendar is connected")]
    NotConnected,

    #[error("the calendar is unavailable: {0}")]
    Unavailable(String),
}

/// Reading and changing a calendar.
///
/// Implementations return events **soonest first**, expanded from any recurrence — a weekly
/// stand-up appearing three times in a three-week span, not once with a repeat rule. The
/// person asks about days, not about rules, so expansion belongs below this line.
#[async_trait]
pub trait CalendarPort: Send + Sync {
    /// Every event overlapping `from`..`to`, soonest first.
    async fn events_between(
        &self,
        from: jiff::Timestamp,
        to: jiff::Timestamp,
    ) -> Result<Vec<CalendarEvent>, CalendarError>;

    /// One event by id, if it is still there.
    async fn get(&self, id: &EventId) -> Result<Option<CalendarEvent>, CalendarError>;

    async fn create(&self, event: &NewEvent) -> Result<CalendarEvent, CalendarError>;

    /// Strike this occurrence off. A series keeps its other occurrences.
    async fn strike_off(&self, id: &EventId) -> Result<(), CalendarError>;

    /// Decline: it stays on the calendar, marked as not attending.
    async fn decline(&self, id: &EventId) -> Result<(), CalendarError>;

    /// Delete for good. Irreversible, and the only operation here that is.
    async fn delete(&self, id: &EventId) -> Result<(), CalendarError>;
}

#[cfg(test)]
mod tests;
