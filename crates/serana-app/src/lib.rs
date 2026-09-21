//! The application layer shared by every frontend: what can be asked for, what comes back,
//! how the object graph is built, and the words the user reads.
//!
//! It sits above `serana-services` and `serana-adapters`, and below the composition roots.
//! Two binaries that each read their own environment and wired their own graph would drift
//! apart within a release; this is where they agree.

pub mod command;
pub mod config;
pub mod respond;
pub mod text;
pub mod wiring;

pub use command::Command;
pub use config::AppConfig;
pub use respond::respond;
pub use wiring::{Reminders, Wiring, build_reminders, build_scheduler};
