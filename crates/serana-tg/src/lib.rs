//! The Telegram frontend: the transport, and nothing that could live without it.
//!
//! Command semantics and every user-facing word are in `serana-ui`, shared with the CLI.
//! What is left here is the teloxide command enum, the notifier, and configuration.

pub mod command;
pub mod config;
pub mod notifier;

pub use command::TelegramCommand;
