//! The Telegram frontend, as a library so its command surface can be driven from
//! integration tests.
//!
//! `main.rs` is the composition root and holds nothing testable; everything here is
//! reachable from `tests/`.

pub mod commands;
pub mod config;
pub mod notifier;
pub mod text;
