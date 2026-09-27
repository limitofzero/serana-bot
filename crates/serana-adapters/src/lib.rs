//! Adapters: everything that talks to the outside world.
//!
//! Each module here implements a port from `serana-domain` and owns all the I/O for it.
//! This crate depends on the domain and nothing else of ours — never on `serana-services`
//! — so the dependency direction is checked by cargo rather than by review.

pub mod calendar;
pub mod clock;
pub mod conversations;
pub mod digests;
pub mod ids;
pub mod llm;
pub mod prices;
pub mod reminders;
mod sqlite;

pub use calendar::{GoogleCalendar, GoogleCalendarConfig};
pub use clock::SystemClock;
pub use conversations::SqliteConversationRepository;
pub use digests::SqliteDigestRepository;
pub use ids::RandomIds;
pub use llm::{OpenAiConfig, OpenAiProvider};
pub use prices::{Chain, CoinGecko, CoinGeckoConfig, DefiLlama, DefiLlamaConfig};
pub use reminders::SqliteReminderRepository;
