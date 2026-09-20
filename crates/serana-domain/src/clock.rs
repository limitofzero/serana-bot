//! Time as a port, so nothing in the domain or services reads the system clock directly.

use std::sync::Arc;

/// The current time.
///
/// Injected rather than called directly so tests can place a conversation at a fixed
/// instant and assert on timestamps without sleeping.
pub trait Clock: Send + Sync {
    fn now(&self) -> jiff::Timestamp;
}

impl<T: Clock + ?Sized> Clock for Arc<T> {
    fn now(&self) -> jiff::Timestamp {
        (**self).now()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(jiff::Timestamp);

    impl Clock for Fixed {
        fn now(&self) -> jiff::Timestamp {
            self.0
        }
    }

    #[test]
    fn a_clock_behind_an_arc_delegates() {
        let clock: Arc<dyn Clock> = Arc::new(Fixed(jiff::Timestamp::UNIX_EPOCH));
        assert_eq!(clock.now(), jiff::Timestamp::UNIX_EPOCH);
    }
}
