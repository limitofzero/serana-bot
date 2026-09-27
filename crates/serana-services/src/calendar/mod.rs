//! The calendar: what is on, putting something on, taking something off — and the
//! conversation that drives it.

mod capability;
mod error;
mod outcome;
mod prompt;
mod service;
mod tools;

pub use capability::Planning;
pub use error::CalendarTurnError;
pub use outcome::CalendarOutcome;
pub use service::{CalendarService, DEFAULT_HORIZON};

/// A conversation about the calendar: the shared loop, with the calendar capability fitted.
///
/// A plain alias rather than a wrapper — unlike reminders, nothing here needs a second
/// surface on top of the loop's own.
pub type CalendarChat<P, L, C, V> = crate::chat::Chat<Planning<P, C>, L, V>;
