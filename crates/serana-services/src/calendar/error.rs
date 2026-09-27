//! Why a calendar turn failed.
//!
//! Each variant exists because the frontend says something different for it. "No calendar
//! is connected" is not an outage and must not be apologised for as one; an event that is
//! gone is not a provider failure. Anything that would only ever be reported belongs in the
//! message of an existing variant.

use serana_domain::calendar::{CalendarError, EventId};
use serana_domain::{LlmError, StorageError};

#[derive(Debug, thiserror::Error)]
pub enum CalendarTurnError {
    /// The request could not be turned into an appointment. The message is written to be
    /// shown to the user, because that is where it ends up.
    #[error("could not understand that: {0}")]
    Unparsable(String),

    /// The model named an event that is not on the calendar — usually one that was there
    /// when the turn started and has since been removed elsewhere.
    #[error("no event with id {0}")]
    NotFound(EventId),

    #[error(transparent)]
    Calendar(#[from] CalendarError),

    #[error(transparent)]
    Llm(#[from] LlmError),

    #[error(transparent)]
    Storage(#[from] StorageError),
}

impl crate::chat::TurnError for CalendarTurnError {
    fn malformed(reason: String) -> Self {
        Self::Unparsable(reason)
    }
}
