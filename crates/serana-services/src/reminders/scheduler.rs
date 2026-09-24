//! The loop that actually delivers reminders.

use std::time::Duration;

use serana_domain::reminder::{Notifier, NotifyError, Reminder, ReminderRepository};
use serana_domain::{Clock, StorageError};

/// What one [`SchedulerService::tick`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TickReport {
    pub delivered: usize,
    /// Reminders switched off because their recipient is permanently unreachable, or
    /// because they can never be rescheduled.
    pub deactivated: usize,
    /// Deliveries that failed transiently and are left due for the next tick.
    pub retrying: usize,
}

impl TickReport {
    pub fn is_quiet(&self) -> bool {
        *self == Self::default()
    }
}

/// Delivers due reminders and advances their schedules.
pub struct SchedulerService<R, N, C> {
    repository: R,
    notifier: N,
    clock: C,
}

impl<R, N, C> SchedulerService<R, N, C>
where
    R: ReminderRepository,
    N: Notifier,
    C: Clock,
{
    pub fn new(repository: R, notifier: N, clock: C) -> Self {
        Self {
            repository,
            notifier,
            clock,
        }
    }

    /// Deliver everything due now.
    ///
    /// A reminder that came due repeatedly while the process was down is delivered **once**
    /// and then rescheduled from the present, not from the moment it was missed. A week of
    /// downtime should not produce seven identical messages.
    pub async fn tick(&self) -> Result<TickReport, StorageError> {
        let now = self.clock.now();
        let mut report = TickReport::default();

        for mut reminder in self.repository.due_at(now).await? {
            match self.notifier.notify(reminder.owner, &reminder, now).await {
                Ok(()) => {
                    if self.advance(&mut reminder, now).await? {
                        report.deactivated += 1;
                    }
                    report.delivered += 1;
                }
                Err(NotifyError::Unreachable(reason)) => {
                    // Retrying forever would mean every tick pays for a delivery that
                    // cannot land. Switch it off and stop.
                    tracing::warn!(
                        reminder = %reminder.id,
                        owner = %reminder.owner,
                        %reason,
                        "deactivating reminder: recipient is unreachable"
                    );
                    reminder.next_fire_at = None;
                    self.repository.put(&reminder).await?;
                    report.deactivated += 1;
                }
                Err(NotifyError::Transport(reason)) => {
                    // Left untouched, so it is still due on the next tick.
                    tracing::warn!(
                        reminder = %reminder.id,
                        %reason,
                        "delivery failed, will retry"
                    );
                    report.retrying += 1;
                }
            }
        }

        Ok(report)
    }

    /// Record the delivery and compute the next occurrence. Returns whether the reminder
    /// is now spent.
    async fn advance(
        &self,
        reminder: &mut Reminder,
        now: jiff::Timestamp,
    ) -> Result<bool, StorageError> {
        let spent = match reminder.mark_fired(now) {
            Ok(next) => next.is_none(),
            Err(error) => {
                // Its zone no longer resolves, so it can never be rescheduled. Leaving
                // `next_fire_at` in the past would redeliver it on every tick forever.
                tracing::error!(
                    reminder = %reminder.id,
                    %error,
                    "deactivating reminder: its schedule can no longer be computed"
                );
                reminder.next_fire_at = None;
                true
            }
        };
        self.repository.put(reminder).await?;
        Ok(spent)
    }

    /// Tick forever, every `interval`.
    ///
    /// Never returns. A storage failure is logged and the loop continues, because a
    /// scheduler that exits on a transient database error stops delivering everything.
    /// Cancel it by dropping the future — `tokio::select!` against a shutdown signal.
    pub async fn run(&self, interval: Duration) -> ! {
        let mut ticker = tokio::time::interval(interval);
        // Catching up on missed ticks would achieve nothing: the work is driven by what is
        // due, not by how many ticks elapsed.
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            ticker.tick().await;
            match self.tick().await {
                Ok(report) if report.is_quiet() => {}
                Ok(report) => tracing::info!(
                    delivered = report.delivered,
                    deactivated = report.deactivated,
                    retrying = report.retrying,
                    "reminder tick"
                ),
                Err(error) => tracing::error!(%error, "reminder tick failed"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serana_domain::reminder::{Recurrence, ReminderId, TimeZoneName, UserId};
    use serana_testkit::{FixedClock, InMemoryReminderRepository, RecordingNotifier};

    use super::*;

    const OWNER: UserId = UserId::new(42);

    fn ts(rfc3339: &str) -> jiff::Timestamp {
        rfc3339.parse().expect("valid instant")
    }

    /// 10:00 in Tbilisi is 06:00 UTC.
    fn reminder(id: &str, recurrence: Recurrence, next_fire_at: Option<&str>) -> Reminder {
        Reminder {
            id: ReminderId::new(id),
            owner: OWNER,
            text: "оформить invoice".into(),
            items: Vec::new(),
            recurrence,
            timezone: TimeZoneName::new("Asia/Tbilisi"),
            created_at: ts("2026-03-01T00:00:00Z"),
            next_fire_at: next_fire_at.map(ts),
            last_fired_at: None,
            acknowledged_through: None,
        }
    }

    fn daily() -> Recurrence {
        Recurrence::Daily {
            at: jiff::civil::time(10, 0, 0, 0),
        }
    }

    fn scheduler(
        reminders: Vec<Reminder>,
        notifier: RecordingNotifier,
        now: &str,
    ) -> SchedulerService<InMemoryReminderRepository, RecordingNotifier, FixedClock> {
        SchedulerService::new(
            InMemoryReminderRepository::seeded(reminders),
            notifier,
            FixedClock::at(now),
        )
    }

    #[tokio::test]
    async fn a_tick_with_nothing_due_does_nothing() {
        let scheduler = scheduler(
            vec![reminder("r1", daily(), Some("2026-03-12T06:00:00Z"))],
            RecordingNotifier::new(),
            "2026-03-10T08:00:00Z",
        );
        let report = scheduler.tick().await.unwrap();
        assert!(report.is_quiet());
        assert!(scheduler.notifier.is_empty());
    }

    #[tokio::test]
    async fn a_due_reminder_is_delivered_and_rescheduled() {
        let scheduler = scheduler(
            vec![reminder("r1", daily(), Some("2026-03-10T06:00:00Z"))],
            RecordingNotifier::new(),
            "2026-03-10T06:00:00Z",
        );

        let report = scheduler.tick().await.unwrap();
        assert_eq!(report.delivered, 1);
        assert_eq!(
            scheduler.notifier.messages_to(OWNER),
            vec!["оформить invoice"]
        );

        let stored = scheduler
            .repository
            .get(&ReminderId::new("r1"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.next_fire_at, Some(ts("2026-03-11T06:00:00Z")));
        assert_eq!(stored.last_fired_at, Some(ts("2026-03-10T06:00:00Z")));
    }

    #[tokio::test]
    async fn a_delivered_reminder_is_not_delivered_again_on_the_next_tick() {
        let scheduler = scheduler(
            vec![reminder("r1", daily(), Some("2026-03-10T06:00:00Z"))],
            RecordingNotifier::new(),
            "2026-03-10T06:00:00Z",
        );
        scheduler.tick().await.unwrap();
        assert!(scheduler.tick().await.unwrap().is_quiet());
        assert_eq!(scheduler.notifier.messages_to(OWNER).len(), 1);
    }

    #[tokio::test]
    async fn a_week_of_downtime_produces_one_message_not_seven() {
        // The reminder came due every day while the process was gone. Replaying each missed
        // occurrence would greet the user with a week of identical messages.
        let scheduler = scheduler(
            vec![reminder("r1", daily(), Some("2026-03-10T06:00:00Z"))],
            RecordingNotifier::new(),
            "2026-03-17T08:00:00Z",
        );

        let report = scheduler.tick().await.unwrap();
        assert_eq!(report.delivered, 1);
        assert_eq!(scheduler.notifier.messages_to(OWNER).len(), 1);

        let stored = scheduler
            .repository
            .get(&ReminderId::new("r1"))
            .await
            .unwrap()
            .unwrap();
        // Rescheduled from now, not from the moment it was missed.
        assert_eq!(stored.next_fire_at, Some(ts("2026-03-18T06:00:00Z")));
    }

    #[tokio::test]
    async fn a_one_off_fires_once_and_goes_quiet() {
        let once = Recurrence::Once {
            at: jiff::civil::date(2026, 3, 10).at(10, 0, 0, 0),
        };
        let scheduler = scheduler(
            vec![reminder("r1", once, Some("2026-03-10T06:00:00Z"))],
            RecordingNotifier::new(),
            "2026-03-10T06:00:00Z",
        );

        let report = scheduler.tick().await.unwrap();
        assert_eq!(report.delivered, 1);
        assert_eq!(
            report.deactivated, 1,
            "a spent one-off is reported as deactivated"
        );

        let stored = scheduler
            .repository
            .get(&ReminderId::new("r1"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.next_fire_at, None);
        assert!(!stored.is_active());
        assert!(scheduler.tick().await.unwrap().is_quiet());
    }

    #[tokio::test]
    async fn an_unreachable_recipient_switches_the_reminder_off() {
        let scheduler = scheduler(
            vec![reminder("r1", daily(), Some("2026-03-10T06:00:00Z"))],
            RecordingNotifier::new().unreachable(OWNER),
            "2026-03-10T06:00:00Z",
        );

        let report = scheduler.tick().await.unwrap();
        assert_eq!(
            report,
            TickReport {
                delivered: 0,
                deactivated: 1,
                retrying: 0
            }
        );

        let stored = scheduler
            .repository
            .get(&ReminderId::new("r1"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.next_fire_at, None);
        assert_eq!(
            stored.last_fired_at, None,
            "nothing was delivered, so nothing is recorded"
        );
        assert!(scheduler.tick().await.unwrap().is_quiet());
    }

    #[tokio::test]
    async fn a_transient_failure_leaves_the_reminder_due_for_the_next_tick() {
        let scheduler = scheduler(
            vec![reminder("r1", daily(), Some("2026-03-10T06:00:00Z"))],
            RecordingNotifier::new().flaky(OWNER),
            "2026-03-10T06:00:00Z",
        );

        let report = scheduler.tick().await.unwrap();
        assert_eq!(
            report,
            TickReport {
                delivered: 0,
                deactivated: 0,
                retrying: 1
            }
        );

        let stored = scheduler
            .repository
            .get(&ReminderId::new("r1"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.next_fire_at,
            Some(ts("2026-03-10T06:00:00Z")),
            "still due"
        );
        assert_eq!(scheduler.tick().await.unwrap().retrying, 1);
    }

    #[tokio::test]
    async fn a_reminder_whose_zone_stopped_resolving_is_switched_off_not_redelivered_forever() {
        let mut broken = reminder("r1", daily(), Some("2026-03-10T06:00:00Z"));
        broken.timezone = TimeZoneName::new("Mars/Olympus_Mons");
        let scheduler = scheduler(
            vec![broken],
            RecordingNotifier::new(),
            "2026-03-10T06:00:00Z",
        );

        let report = scheduler.tick().await.unwrap();
        assert_eq!(report.delivered, 1, "the message still went out");
        assert_eq!(report.deactivated, 1);

        let stored = scheduler
            .repository
            .get(&ReminderId::new("r1"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.next_fire_at, None);
        assert!(scheduler.tick().await.unwrap().is_quiet());
    }

    #[tokio::test]
    async fn one_tick_serves_every_owner() {
        let other = UserId::new(7);
        let mut theirs = reminder("r2", daily(), Some("2026-03-10T06:00:00Z"));
        theirs.owner = other;
        theirs.text = "walk the dog".into();

        let scheduler = scheduler(
            vec![
                reminder("r1", daily(), Some("2026-03-10T06:00:00Z")),
                theirs,
            ],
            RecordingNotifier::new(),
            "2026-03-10T06:00:00Z",
        );

        assert_eq!(scheduler.tick().await.unwrap().delivered, 2);
        assert_eq!(
            scheduler.notifier.messages_to(OWNER),
            vec!["оформить invoice"]
        );
        assert_eq!(scheduler.notifier.messages_to(other), vec!["walk the dog"]);
    }

    #[tokio::test]
    async fn one_unreachable_recipient_does_not_stop_the_others() {
        let blocked = UserId::new(7);
        let mut theirs = reminder("r2", daily(), Some("2026-03-10T05:00:00Z"));
        theirs.owner = blocked;

        let scheduler = scheduler(
            vec![
                reminder("r1", daily(), Some("2026-03-10T06:00:00Z")),
                theirs,
            ],
            RecordingNotifier::new().unreachable(blocked),
            "2026-03-10T06:00:00Z",
        );

        let report = scheduler.tick().await.unwrap();
        assert_eq!(
            report,
            TickReport {
                delivered: 1,
                deactivated: 1,
                retrying: 0
            }
        );
        assert_eq!(scheduler.notifier.messages_to(OWNER).len(), 1);
    }
}
