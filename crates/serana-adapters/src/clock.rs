//! The system clock.

use serana_domain::Clock;

/// Reads the host clock. The only implementation that does.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> jiff::Timestamp {
        jiff::Timestamp::now()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_clock_moves_forward() {
        let clock = SystemClock;
        let first = clock.now();
        let second = clock.now();
        assert!(second >= first);
    }

    #[test]
    fn the_system_clock_is_somewhere_in_this_century() {
        // A sanity check against a misconfigured container clock, not a precise assertion.
        let now = SystemClock.now();
        assert!(now > "2020-01-01T00:00:00Z".parse().unwrap());
        assert!(now < "2100-01-01T00:00:00Z".parse().unwrap());
    }
}
