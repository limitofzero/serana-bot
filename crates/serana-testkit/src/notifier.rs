//! A notifier that records deliveries instead of sending them.

use std::sync::Mutex;

use async_trait::async_trait;
use serana_domain::reminder::{Notifier, NotifyError, Reminder, UserId};

/// Captures every delivery, and can be told to fail.
pub struct RecordingNotifier {
    delivered: Mutex<Vec<(UserId, String)>>,
    /// Recipients that reject delivery, and how.
    failing: Mutex<Vec<(UserId, bool)>>,
}

impl Default for RecordingNotifier {
    fn default() -> Self {
        Self::new()
    }
}

impl RecordingNotifier {
    pub fn new() -> Self {
        Self {
            delivered: Mutex::new(Vec::new()),
            failing: Mutex::new(Vec::new()),
        }
    }

    /// Make delivery to `owner` fail permanently, as a blocked bot would.
    pub fn unreachable(self, owner: UserId) -> Self {
        self.failing
            .lock()
            .expect("failing mutex poisoned")
            .push((owner, true));
        self
    }

    /// Make delivery to `owner` fail transiently.
    pub fn flaky(self, owner: UserId) -> Self {
        self.failing
            .lock()
            .expect("failing mutex poisoned")
            .push((owner, false));
        self
    }

    /// Everything delivered so far, in order.
    pub fn delivered(&self) -> Vec<(UserId, String)> {
        self.delivered
            .lock()
            .expect("delivered mutex poisoned")
            .clone()
    }

    /// Message bodies sent to `owner`.
    pub fn messages_to(&self, owner: UserId) -> Vec<String> {
        self.delivered()
            .into_iter()
            .filter(|(to, _)| *to == owner)
            .map(|(_, text)| text)
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.delivered
            .lock()
            .expect("delivered mutex poisoned")
            .is_empty()
    }
}

#[async_trait]
impl Notifier for RecordingNotifier {
    async fn notify(
        &self,
        owner: UserId,
        reminder: &Reminder,
        _now: jiff::Timestamp,
    ) -> Result<(), NotifyError> {
        let permanent = self
            .failing
            .lock()
            .expect("failing mutex poisoned")
            .iter()
            .find(|(to, _)| *to == owner)
            .map(|(_, permanent)| *permanent);

        match permanent {
            Some(true) => Err(NotifyError::Unreachable(format!("{owner} blocked the bot"))),
            Some(false) => Err(NotifyError::Transport("network".into())),
            None => {
                self.delivered
                    .lock()
                    .expect("delivered mutex poisoned")
                    // Recorded as the text alone: what a frontend renders around it is
                    // presentation, and no service should be asserting on that.
                    .push((owner, reminder.text.clone()));
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serana_domain::reminder::{Recurrence, ReminderId, TimeZoneName};

    /// A reminder whose text is `text`; the rest is scenery.
    fn reminder(text: &str) -> Reminder {
        Reminder {
            id: ReminderId::new("r1"),
            owner: UserId::new(1),
            text: text.into(),
            items: Vec::new(),
            recurrence: Recurrence::Daily {
                at: jiff::civil::time(9, 0, 0, 0),
            },
            timezone: TimeZoneName::new("Asia/Tbilisi"),
            created_at: jiff::Timestamp::UNIX_EPOCH,
            next_fire_at: None,
            last_fired_at: None,
            acknowledged_through: None,
        }
    }

    fn now() -> jiff::Timestamp {
        jiff::Timestamp::UNIX_EPOCH
    }

    use super::*;

    #[tokio::test]
    async fn deliveries_are_recorded_in_order() {
        let notifier = RecordingNotifier::new();
        assert!(notifier.is_empty());
        notifier
            .notify(UserId::new(1), &reminder("first"), now())
            .await
            .unwrap();
        notifier
            .notify(UserId::new(1), &reminder("second"), now())
            .await
            .unwrap();
        assert_eq!(
            notifier.messages_to(UserId::new(1)),
            vec!["first", "second"]
        );
    }

    #[tokio::test]
    async fn deliveries_are_separated_by_recipient() {
        let notifier = RecordingNotifier::new();
        notifier
            .notify(UserId::new(1), &reminder("for one"), now())
            .await
            .unwrap();
        notifier
            .notify(UserId::new(2), &reminder("for two"), now())
            .await
            .unwrap();
        assert_eq!(notifier.messages_to(UserId::new(1)), vec!["for one"]);
        assert_eq!(notifier.messages_to(UserId::new(2)), vec!["for two"]);
    }

    #[tokio::test]
    async fn an_unreachable_recipient_fails_permanently_and_records_nothing() {
        let notifier = RecordingNotifier::new().unreachable(UserId::new(1));
        let err = notifier
            .notify(UserId::new(1), &reminder("hi"), now())
            .await
            .unwrap_err();
        assert!(matches!(err, NotifyError::Unreachable(_)));
        assert!(notifier.is_empty());
    }

    #[tokio::test]
    async fn a_flaky_recipient_fails_transiently() {
        let notifier = RecordingNotifier::new().flaky(UserId::new(1));
        let err = notifier
            .notify(UserId::new(1), &reminder("hi"), now())
            .await
            .unwrap_err();
        assert!(matches!(err, NotifyError::Transport(_)));
    }
}
