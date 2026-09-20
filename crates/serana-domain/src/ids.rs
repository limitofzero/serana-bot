//! Identifier generation as a port.
//!
//! A service that calls a random number generator directly cannot be asserted on, so this
//! is injected like the clock is.

use std::sync::Arc;

/// Mints identifiers for new entities.
///
/// Identifiers reach users — `/reminder_delete a3f9k2` — so implementations favour short,
/// unambiguous strings over UUIDs, which nobody will retype correctly.
pub trait IdGenerator: Send + Sync {
    fn generate(&self) -> String;
}

impl<T: IdGenerator + ?Sized> IdGenerator for Arc<T> {
    fn generate(&self) -> String {
        (**self).generate()
    }
}
