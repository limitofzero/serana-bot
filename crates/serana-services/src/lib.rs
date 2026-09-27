//! Services: the orchestration between ports.
//!
//! Everything here takes its dependencies as ports from `serana-domain` and can therefore
//! be constructed entirely from `serana-testkit` fakes. A service that reaches for an HTTP
//! client, a database handle or the system clock has broken the layering — cargo enforces
//! that this crate cannot see `serana-adapters`.

pub mod calendar;
pub mod chat;
pub mod prices;
pub mod reminders;
mod schedule;

pub use calendar::{CalendarChat, CalendarOutcome, CalendarService, CalendarTurnError, Planning};
pub use chat::{Capability, Chat, ChatConfig, DEFAULT_COMPACT_ABOVE_TOKENS, TurnError};
pub use prices::{
    DigestReport, DigestScheduler, PriceChat, PriceConfig, PriceOutcome, PriceService,
    PriceTurnError, Watching,
};
pub use reminders::{
    ChatService, ReminderError, ReminderOutcome, ReminderService, SchedulerService, TickReport,
};
