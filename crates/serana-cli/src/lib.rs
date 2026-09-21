//! The terminal frontend.
//!
//! It manages reminders; it does not deliver them. Delivery belongs to the long-running
//! bot, and a second scheduler over the same database would send every reminder twice.

pub mod input;

pub use input::{Input, parse};
