//! In-memory fakes for testing services without touching the network, a database or a
//! clock.
//!
//! This crate is a dev-dependency everywhere. Nothing in it is reachable from a release
//! build, and nothing in it may be used to stand in for an adapter's own tests: an adapter
//! is tested against the real thing it adapts (a mock HTTP server, a temporary directory),
//! never against a fake of itself.

pub mod clock;
pub mod llm;
pub mod notifier;
pub mod reminders;
pub mod tools;

pub use clock::FixedClock;
pub use llm::ScriptedLlm;
pub use notifier::RecordingNotifier;
pub use reminders::InMemoryReminderRepository;
pub use tools::{EchoTool, FailingTool};

pub mod ids;
pub use ids::SequentialIds;
