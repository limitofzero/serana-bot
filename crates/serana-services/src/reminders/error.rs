//! Why a reminder turn failed.
//!
//! Each variant exists because a caller behaves differently for it — a different reply, a
//! different recovery. Anything that would only ever be reported belongs in the message of
//! an existing variant, not in a new one.

use serana_domain::reminder::ReminderId;
use serana_domain::{LlmError, StorageError};

#[derive(Debug, thiserror::Error)]
pub enum ReminderError {
    /// The request could not be turned into a schedule. The message is written to be shown
    /// to the user, because that is where it ends up.
    #[error("could not understand the schedule: {0}")]
    Unparsable(String),

    /// The schedule is well-formed but has no occurrence ahead of now — a one-off in the
    /// past, or a monthly reminder for a day that does not exist.
    #[error("that schedule has no future occurrence")]
    NeverFires,

    #[error("no reminder with id {0}")]
    NotFound(ReminderId),

    /// Asked to tick something off a reminder that has no checklist.
    #[error("reminder {0} has no checklist")]
    NoChecklist(ReminderId),

    #[error(transparent)]
    Llm(#[from] LlmError),

    #[error(transparent)]
    Storage(#[from] StorageError),
}
