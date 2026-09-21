//! Services: the orchestration between ports.
//!
//! Everything here takes its dependencies as ports from `serana-domain` and can therefore
//! be constructed entirely from `serana-testkit` fakes. A service that reaches for an HTTP
//! client, a database handle or the system clock has broken the layering — cargo enforces
//! that this crate cannot see `serana-adapters`.

pub mod reminders;

pub use reminders::{
    DEFAULT_COMPACT_ABOVE_TOKENS, ReminderConfig, ReminderError, ReminderOutcome, ReminderService,
    SchedulerService, TickReport,
};
