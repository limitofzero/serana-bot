//! Crypto prices: the standing order, the daily send, and the conversation that changes it.

mod capability;
mod config;
mod error;
mod outcome;
mod prompt;
mod scheduler;
mod service;
mod tools;

pub use capability::Watching;
pub use config::{DEFAULT_CACHE_TTL, PriceConfig};
pub use error::PriceTurnError;
pub use outcome::{PriceOutcome, WatchlistEdit};
pub use scheduler::{DigestReport, DigestScheduler};
pub use service::PriceService;

/// A conversation about prices: the shared loop, with the prices capability fitted.
pub type PriceChat<S, R, L, C, V> = crate::chat::Chat<Watching<S, R, C>, L, V>;
