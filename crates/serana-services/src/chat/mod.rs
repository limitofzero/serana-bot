//! The conversation loop, and what it takes to give it a subject.

mod capability;
mod config;
mod session;

pub use capability::{Capability, TurnError};
pub use config::{ChatConfig, DEFAULT_COMPACT_ABOVE_TOKENS};
pub use session::Chat;

#[cfg(test)]
mod tests;
