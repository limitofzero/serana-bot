//! A clock tests control.

use std::sync::Mutex;

use serana_domain::Clock;

/// A clock that returns whatever instant it was last set to.
///
/// Tests that need elapsed time advance it explicitly rather than sleeping, so a suite
/// covering a month of reminders still runs in milliseconds.
#[derive(Debug)]
pub struct FixedClock(Mutex<jiff::Timestamp>);

impl FixedClock {
    pub fn new(now: jiff::Timestamp) -> Self {
        Self(Mutex::new(now))
    }

    /// A clock at the Unix epoch, for tests that do not care when they are.
    pub fn epoch() -> Self {
        Self::new(jiff::Timestamp::UNIX_EPOCH)
    }

    /// Parse an RFC 3339 instant, panicking on a malformed one — this is test-only setup.
    pub fn at(rfc3339: &str) -> Self {
        Self::new(
            rfc3339
                .parse()
                .expect("test clock needs a valid RFC 3339 instant"),
        )
    }

    pub fn set(&self, now: jiff::Timestamp) {
        *self.0.lock().expect("clock mutex poisoned") = now;
    }

    pub fn advance(&self, span: jiff::Span) {
        let mut now = self.0.lock().expect("clock mutex poisoned");
        *now = now.checked_add(span).expect("test clock overflowed");
    }
}

impl Clock for FixedClock {
    fn now(&self) -> jiff::Timestamp {
        *self.0.lock().expect("clock mutex poisoned")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fixed_clock_does_not_move_on_its_own() {
        let clock = FixedClock::at("2026-03-10T08:00:00Z");
        assert_eq!(clock.now(), clock.now());
    }

    #[test]
    fn advancing_moves_it_by_exactly_the_span() {
        let clock = FixedClock::at("2026-03-10T08:00:00Z");
        clock.advance(jiff::Span::new().hours(2));
        assert_eq!(clock.now().to_string(), "2026-03-10T10:00:00Z");
    }

    #[test]
    fn setting_replaces_the_instant() {
        let clock = FixedClock::epoch();
        clock.set("2026-03-10T08:00:00Z".parse().unwrap());
        assert_eq!(clock.now().to_string(), "2026-03-10T08:00:00Z");
    }
}
