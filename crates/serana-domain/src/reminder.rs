//! Reminders: what to say, when to say it again, and to whom.
//!
//! Recurrence is modelled as an enum over jiff's date arithmetic rather than as a cron
//! expression. Cron would mean a second time library in the tree, cannot express every
//! pattern a person actually asks for, and is lossy in the direction that matters most
//! here: a stored `0 10 20 * *` has to be translated back into words before it can be
//! shown to the user, whereas a [`Recurrence`] renders itself.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::StorageError;

/// Whoever owns a reminder. Currently a Telegram user id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct UserId(i64);

impl UserId {
    pub const fn new(id: i64) -> Self {
        Self(id)
    }

    pub const fn get(&self) -> i64 {
        self.0
    }
}

impl std::fmt::Display for UserId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReminderId(String);

impl ReminderId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ReminderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// An IANA time zone name, kept as text so it survives a round trip through storage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeZoneName(String);

impl TimeZoneName {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Resolve against the tzdb.
    ///
    /// Fallible because a zone name read back from storage may no longer exist — tzdb drops
    /// and renames zones — and a reminder that silently moved to UTC is worse than one that
    /// reports the problem.
    pub fn resolve(&self) -> Result<jiff::tz::TimeZone, StorageError> {
        jiff::tz::TimeZone::get(&self.0)
            .map_err(|e| StorageError::Corrupt(format!("unknown time zone {:?}: {e}", self.0)))
    }
}

impl std::fmt::Display for TimeZoneName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Day of the week, mirrored from jiff so it can be serialised by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Weekday {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

impl From<Weekday> for jiff::civil::Weekday {
    fn from(day: Weekday) -> Self {
        match day {
            Weekday::Monday => Self::Monday,
            Weekday::Tuesday => Self::Tuesday,
            Weekday::Wednesday => Self::Wednesday,
            Weekday::Thursday => Self::Thursday,
            Weekday::Friday => Self::Friday,
            Weekday::Saturday => Self::Saturday,
            Weekday::Sunday => Self::Sunday,
        }
    }
}

/// When a reminder fires.
///
/// Times are wall-clock times in the reminder's own zone, not instants: "every day at
/// 10:00" means 10:00 local, before and after a daylight-saving transition alike.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Recurrence {
    /// Fires once, at a wall-clock instant in the reminder's zone, then goes inactive.
    Once {
        at: jiff::civil::DateTime,
    },
    Daily {
        at: jiff::civil::Time,
    },
    Weekly {
        weekday: Weekday,
        at: jiff::civil::Time,
    },
    /// `day` is 1-31. Months without that day are **skipped**, not clamped: a reminder for
    /// the 31st does not fire on 28 February. Clamping would fire it on a day the user did
    /// not ask for, which is the worse failure for something like an invoice deadline.
    Monthly {
        day: i8,
        at: jiff::civil::Time,
    },
}

/// How far ahead [`Recurrence::next_occurrence_after`] will search for a monthly reminder
/// before giving up. Only `day == 30` or `31` can skip months at all, and never more than
/// a few in a row, so anything beyond this is a corrupt `day` value.
const MAX_MONTHS_SEARCHED: usize = 60;

impl Recurrence {
    /// The first time this fires strictly after `after`, or `None` for a
    /// [`Recurrence::Once`] whose moment has passed.
    ///
    /// Strictly after, so a scheduler that wakes at the exact firing instant and then
    /// recomputes does not return the same moment forever.
    ///
    /// Wall-clock times that a daylight-saving transition makes ambiguous or non-existent
    /// are resolved with jiff's `compatible` strategy: a time in a spring-forward gap fires
    /// at the following instant, and a time repeated by a fall-back fires on its first
    /// occurrence.
    pub fn next_occurrence_after(
        &self,
        after: jiff::Timestamp,
        tz: &jiff::tz::TimeZone,
    ) -> Option<jiff::Timestamp> {
        let today = after.to_zoned(tz.clone()).date();
        match self {
            Self::Once { at } => {
                let ts = to_instant(*at, tz)?;
                (ts > after).then_some(ts)
            }
            Self::Daily { at } => (0..=1).find_map(|offset| {
                let date = today.checked_add(jiff::Span::new().days(offset)).ok()?;
                candidate_after(date, *at, tz, after)
            }),
            Self::Weekly { weekday, at } => (0..=7).find_map(|offset| {
                let date = today.checked_add(jiff::Span::new().days(offset)).ok()?;
                (date.weekday() == (*weekday).into())
                    .then(|| candidate_after(date, *at, tz, after))
                    .flatten()
            }),
            Self::Monthly { day, at } => {
                let day = *day;
                if !(1..=31).contains(&day) {
                    return None;
                }
                let first = today.first_of_month();
                (0..MAX_MONTHS_SEARCHED).find_map(|offset| {
                    let month = first
                        .checked_add(jiff::Span::new().months(offset as i64))
                        .ok()?;
                    // Skip, do not clamp: the user asked for the 31st.
                    if day > month.days_in_month() {
                        return None;
                    }
                    let date = month.with().day(day).build().ok()?;
                    candidate_after(date, *at, tz, after)
                })
            }
        }
    }

    /// Whether this recurrence can fire more than once.
    pub fn is_recurring(&self) -> bool {
        !matches!(self, Self::Once { .. })
    }
}

/// Resolve a wall-clock date and time in `tz` to an instant.
fn to_instant(at: jiff::civil::DateTime, tz: &jiff::tz::TimeZone) -> Option<jiff::Timestamp> {
    at.to_zoned(tz.clone()).ok().map(|z| z.timestamp())
}

/// The instant `date` at `time` in `tz`, if it falls strictly after `after`.
fn candidate_after(
    date: jiff::civil::Date,
    time: jiff::civil::Time,
    tz: &jiff::tz::TimeZone,
    after: jiff::Timestamp,
) -> Option<jiff::Timestamp> {
    let ts = to_instant(date.to_datetime(time), tz)?;
    (ts > after).then_some(ts)
}

/// A scheduled reminder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reminder {
    pub id: ReminderId,
    pub owner: UserId,
    /// What to send. Stored as the user's own words.
    pub text: String,
    pub recurrence: Recurrence,
    pub timezone: TimeZoneName,
    pub created_at: jiff::Timestamp,
    /// When this next fires. `None` means it never will again — a spent
    /// [`Recurrence::Once`], or a cancelled reminder.
    ///
    /// Denormalised onto the row so the scheduler's "what is due" query is one indexed
    /// lookup instead of evaluating every recurrence on every tick.
    pub next_fire_at: Option<jiff::Timestamp>,
    pub last_fired_at: Option<jiff::Timestamp>,
}

impl Reminder {
    /// Recompute [`Reminder::next_fire_at`] from `now`.
    ///
    /// Called on creation and after every delivery. Returns the new value.
    pub fn reschedule(
        &mut self,
        now: jiff::Timestamp,
    ) -> Result<Option<jiff::Timestamp>, StorageError> {
        let tz = self.timezone.resolve()?;
        self.next_fire_at = self.recurrence.next_occurrence_after(now, &tz);
        Ok(self.next_fire_at)
    }

    /// Whether this reminder has a future firing time.
    pub fn is_active(&self) -> bool {
        self.next_fire_at.is_some()
    }

    /// Mark a delivery at `now` and compute the following occurrence.
    pub fn mark_fired(
        &mut self,
        now: jiff::Timestamp,
    ) -> Result<Option<jiff::Timestamp>, StorageError> {
        self.last_fired_at = Some(now);
        self.reschedule(now)
    }
}

/// Persistence for reminders.
#[async_trait]
pub trait ReminderRepository: Send + Sync {
    async fn get(&self, id: &ReminderId) -> Result<Option<Reminder>, StorageError>;

    /// Insert or replace.
    async fn put(&self, reminder: &Reminder) -> Result<(), StorageError>;

    async fn delete(&self, id: &ReminderId) -> Result<(), StorageError>;

    /// Every reminder owned by `owner`, soonest first, including spent ones.
    async fn list_for_owner(&self, owner: UserId) -> Result<Vec<Reminder>, StorageError>;

    /// Reminders whose `next_fire_at` is at or before `now`.
    ///
    /// The scheduler's hot path: implementations index `next_fire_at` rather than scanning.
    async fn due_at(&self, now: jiff::Timestamp) -> Result<Vec<Reminder>, StorageError>;
}

/// Delivers a message to a user out of band — the push half of a reminder.
#[async_trait]
pub trait Notifier: Send + Sync {
    async fn notify(&self, owner: UserId, text: &str) -> Result<(), NotifyError>;
}

#[derive(Debug, thiserror::Error)]
pub enum NotifyError {
    /// The user cannot be reached and never will be — blocked the bot, deleted the chat.
    /// The scheduler deactivates reminders rather than retrying these forever.
    #[error("recipient unreachable: {0}")]
    Unreachable(String),

    #[error("delivery failed: {0}")]
    Transport(String),
}

#[async_trait]
impl<T: ReminderRepository + ?Sized> ReminderRepository for Arc<T> {
    async fn get(&self, id: &ReminderId) -> Result<Option<Reminder>, StorageError> {
        (**self).get(id).await
    }

    async fn put(&self, reminder: &Reminder) -> Result<(), StorageError> {
        (**self).put(reminder).await
    }

    async fn delete(&self, id: &ReminderId) -> Result<(), StorageError> {
        (**self).delete(id).await
    }

    async fn list_for_owner(&self, owner: UserId) -> Result<Vec<Reminder>, StorageError> {
        (**self).list_for_owner(owner).await
    }

    async fn due_at(&self, now: jiff::Timestamp) -> Result<Vec<Reminder>, StorageError> {
        (**self).due_at(now).await
    }
}

#[async_trait]
impl<T: Notifier + ?Sized> Notifier for Arc<T> {
    async fn notify(&self, owner: UserId, text: &str) -> Result<(), NotifyError> {
        (**self).notify(owner, text).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::{date, time};

    /// Georgia has no daylight saving, so this zone isolates recurrence logic from DST.
    fn tbilisi() -> jiff::tz::TimeZone {
        jiff::tz::TimeZone::get("Asia/Tbilisi").unwrap()
    }

    /// Has DST, so transitions are exercised even though the default zone does not.
    fn berlin() -> jiff::tz::TimeZone {
        jiff::tz::TimeZone::get("Europe/Berlin").unwrap()
    }

    fn at(y: i16, m: i8, d: i8, h: i8, min: i8, tz: &jiff::tz::TimeZone) -> jiff::Timestamp {
        date(y, m, d)
            .at(h, min, 0, 0)
            .to_zoned(tz.clone())
            .unwrap()
            .timestamp()
    }

    fn local(ts: jiff::Timestamp, tz: &jiff::tz::TimeZone) -> jiff::civil::DateTime {
        ts.to_zoned(tz.clone()).datetime()
    }

    #[test]
    fn a_daily_reminder_fires_later_today_when_the_time_has_not_passed() {
        let tz = tbilisi();
        let r = Recurrence::Daily {
            at: time(10, 0, 0, 0),
        };
        let next = r
            .next_occurrence_after(at(2026, 3, 10, 8, 0, &tz), &tz)
            .unwrap();
        assert_eq!(local(next, &tz), date(2026, 3, 10).at(10, 0, 0, 0));
    }

    #[test]
    fn a_daily_reminder_rolls_to_tomorrow_once_the_time_has_passed() {
        let tz = tbilisi();
        let r = Recurrence::Daily {
            at: time(10, 0, 0, 0),
        };
        let next = r
            .next_occurrence_after(at(2026, 3, 10, 11, 0, &tz), &tz)
            .unwrap();
        assert_eq!(local(next, &tz), date(2026, 3, 11).at(10, 0, 0, 0));
    }

    #[test]
    fn the_exact_firing_instant_yields_the_next_one_not_the_same_one() {
        // Otherwise a scheduler that wakes precisely on time reschedules to now, forever.
        let tz = tbilisi();
        let r = Recurrence::Daily {
            at: time(10, 0, 0, 0),
        };
        let now = at(2026, 3, 10, 10, 0, &tz);
        let next = r.next_occurrence_after(now, &tz).unwrap();
        assert!(next > now);
        assert_eq!(local(next, &tz), date(2026, 3, 11).at(10, 0, 0, 0));
    }

    #[test]
    fn a_weekly_reminder_finds_the_next_matching_weekday() {
        let tz = tbilisi();
        // 2026-03-10 is a Tuesday.
        let r = Recurrence::Weekly {
            weekday: Weekday::Friday,
            at: time(9, 30, 0, 0),
        };
        let next = r
            .next_occurrence_after(at(2026, 3, 10, 12, 0, &tz), &tz)
            .unwrap();
        assert_eq!(local(next, &tz), date(2026, 3, 13).at(9, 30, 0, 0));
    }

    #[test]
    fn a_weekly_reminder_on_today_still_fires_today_if_the_time_is_ahead() {
        let tz = tbilisi();
        let r = Recurrence::Weekly {
            weekday: Weekday::Tuesday,
            at: time(18, 0, 0, 0),
        };
        let next = r
            .next_occurrence_after(at(2026, 3, 10, 12, 0, &tz), &tz)
            .unwrap();
        assert_eq!(local(next, &tz), date(2026, 3, 10).at(18, 0, 0, 0));
    }

    #[test]
    fn a_weekly_reminder_on_today_rolls_a_full_week_once_the_time_has_passed() {
        let tz = tbilisi();
        let r = Recurrence::Weekly {
            weekday: Weekday::Tuesday,
            at: time(9, 0, 0, 0),
        };
        let next = r
            .next_occurrence_after(at(2026, 3, 10, 12, 0, &tz), &tz)
            .unwrap();
        assert_eq!(local(next, &tz), date(2026, 3, 17).at(9, 0, 0, 0));
    }

    #[test]
    fn the_invoice_reminder_from_the_brief_lands_on_the_twentieth() {
        let tz = tbilisi();
        let r = Recurrence::Monthly {
            day: 20,
            at: time(10, 0, 0, 0),
        };
        let next = r
            .next_occurrence_after(at(2026, 3, 5, 9, 0, &tz), &tz)
            .unwrap();
        assert_eq!(local(next, &tz), date(2026, 3, 20).at(10, 0, 0, 0));

        // And from just after it, the same day next month.
        let after = r
            .next_occurrence_after(at(2026, 3, 20, 10, 1, &tz), &tz)
            .unwrap();
        assert_eq!(local(after, &tz), date(2026, 4, 20).at(10, 0, 0, 0));
    }

    #[test]
    fn a_monthly_reminder_skips_months_without_that_day_rather_than_clamping() {
        let tz = tbilisi();
        let r = Recurrence::Monthly {
            day: 31,
            at: time(10, 0, 0, 0),
        };
        // From 1 February 2026: February has 28 days, so the next is 31 March.
        let next = r
            .next_occurrence_after(at(2026, 2, 1, 0, 0, &tz), &tz)
            .unwrap();
        assert_eq!(local(next, &tz), date(2026, 3, 31).at(10, 0, 0, 0));
    }

    #[test]
    fn a_monthly_reminder_on_the_twenty_ninth_finds_a_leap_day() {
        let tz = tbilisi();
        let r = Recurrence::Monthly {
            day: 29,
            at: time(8, 0, 0, 0),
        };
        // 2028 is a leap year.
        let next = r
            .next_occurrence_after(at(2028, 2, 1, 0, 0, &tz), &tz)
            .unwrap();
        assert_eq!(local(next, &tz), date(2028, 2, 29).at(8, 0, 0, 0));
    }

    #[test]
    fn a_monthly_reminder_with_an_impossible_day_never_fires() {
        let tz = tbilisi();
        for day in [0, 32, -1] {
            let r = Recurrence::Monthly {
                day,
                at: time(10, 0, 0, 0),
            };
            assert_eq!(
                r.next_occurrence_after(at(2026, 3, 1, 0, 0, &tz), &tz),
                None
            );
        }
    }

    #[test]
    fn a_once_reminder_fires_then_stops() {
        let tz = tbilisi();
        let r = Recurrence::Once {
            at: date(2026, 3, 20).at(10, 0, 0, 0),
        };
        let next = r
            .next_occurrence_after(at(2026, 3, 1, 0, 0, &tz), &tz)
            .unwrap();
        assert_eq!(local(next, &tz), date(2026, 3, 20).at(10, 0, 0, 0));
        assert_eq!(
            r.next_occurrence_after(at(2026, 3, 20, 10, 1, &tz), &tz),
            None
        );
        assert!(!r.is_recurring());
    }

    #[test]
    fn a_wall_clock_time_survives_a_spring_forward() {
        // Berlin skips 02:00-03:00 on 2026-03-29. A 10:00 reminder is unaffected, and the
        // interval across the transition is 23 hours, not 24 — the point of using wall
        // clock times rather than fixed intervals.
        let tz = berlin();
        let r = Recurrence::Daily {
            at: time(10, 0, 0, 0),
        };
        let before = r
            .next_occurrence_after(at(2026, 3, 28, 12, 0, &tz), &tz)
            .unwrap();
        assert_eq!(local(before, &tz), date(2026, 3, 29).at(10, 0, 0, 0));
        let previous = at(2026, 3, 28, 10, 0, &tz);
        let elapsed = (before - previous).total(jiff::Unit::Hour).unwrap();
        assert_eq!(elapsed, 23.0, "the day of the transition is an hour short");
    }

    #[test]
    fn a_time_inside_a_spring_forward_gap_still_fires() {
        // 02:30 does not exist in Berlin on 2026-03-29; it must not silently vanish.
        let tz = berlin();
        let r = Recurrence::Daily {
            at: time(2, 30, 0, 0),
        };
        let next = r
            .next_occurrence_after(at(2026, 3, 29, 0, 0, &tz), &tz)
            .unwrap();
        assert_eq!(local(next, &tz), date(2026, 3, 29).at(3, 30, 0, 0));
    }

    #[test]
    fn a_time_repeated_by_a_fall_back_fires_on_its_first_occurrence() {
        // Berlin repeats 02:00-03:00 on 2026-10-25; the earlier instant wins.
        let tz = berlin();
        let r = Recurrence::Daily {
            at: time(2, 30, 0, 0),
        };
        let next = r
            .next_occurrence_after(at(2026, 10, 25, 0, 0, &tz), &tz)
            .unwrap();
        assert_eq!(local(next, &tz), date(2026, 10, 25).at(2, 30, 0, 0));
        // The first 02:30 is still summer time, so it is 00:30 UTC.
        assert_eq!(next.to_string(), "2026-10-25T00:30:00Z");
    }

    #[test]
    fn the_same_wall_clock_time_is_a_different_instant_in_a_different_zone() {
        let r = Recurrence::Daily {
            at: time(10, 0, 0, 0),
        };
        let now = jiff::Timestamp::from_second(1_772_000_000).unwrap();
        let tbilisi_next = r.next_occurrence_after(now, &tbilisi()).unwrap();
        let berlin_next = r.next_occurrence_after(now, &berlin()).unwrap();
        assert_ne!(tbilisi_next, berlin_next);
    }

    #[test]
    fn an_unknown_time_zone_is_reported_rather_than_defaulted() {
        assert!(TimeZoneName::new("Asia/Tbilisi").resolve().is_ok());
        let err = TimeZoneName::new("Mars/Olympus_Mons")
            .resolve()
            .unwrap_err();
        assert!(matches!(err, StorageError::Corrupt(_)), "{err:?}");
    }

    fn reminder(recurrence: Recurrence) -> Reminder {
        Reminder {
            id: ReminderId::new("r1"),
            owner: UserId::new(42),
            text: "оформить invoice".into(),
            recurrence,
            timezone: TimeZoneName::new("Asia/Tbilisi"),
            created_at: jiff::Timestamp::UNIX_EPOCH,
            next_fire_at: None,
            last_fired_at: None,
        }
    }

    #[test]
    fn rescheduling_fills_in_the_next_firing_time() {
        let tz = tbilisi();
        let mut r = reminder(Recurrence::Monthly {
            day: 20,
            at: time(10, 0, 0, 0),
        });
        assert!(!r.is_active());
        let next = r.reschedule(at(2026, 3, 5, 9, 0, &tz)).unwrap().unwrap();
        assert_eq!(local(next, &tz), date(2026, 3, 20).at(10, 0, 0, 0));
        assert!(r.is_active());
    }

    #[test]
    fn firing_advances_the_schedule_and_records_the_delivery() {
        let tz = tbilisi();
        let mut r = reminder(Recurrence::Monthly {
            day: 20,
            at: time(10, 0, 0, 0),
        });
        let fired_at = at(2026, 3, 20, 10, 0, &tz);
        let next = r.mark_fired(fired_at).unwrap().unwrap();
        assert_eq!(r.last_fired_at, Some(fired_at));
        assert_eq!(local(next, &tz), date(2026, 4, 20).at(10, 0, 0, 0));
    }

    #[test]
    fn firing_a_one_off_leaves_it_inactive() {
        let tz = tbilisi();
        let mut r = reminder(Recurrence::Once {
            at: date(2026, 3, 20).at(10, 0, 0, 0),
        });
        assert_eq!(r.mark_fired(at(2026, 3, 20, 10, 0, &tz)).unwrap(), None);
        assert!(!r.is_active());
        assert!(r.last_fired_at.is_some());
    }

    #[test]
    fn a_reminder_in_a_broken_zone_reports_instead_of_rescheduling() {
        let mut r = reminder(Recurrence::Daily {
            at: time(10, 0, 0, 0),
        });
        r.timezone = TimeZoneName::new("Mars/Olympus_Mons");
        assert!(r.reschedule(jiff::Timestamp::UNIX_EPOCH).is_err());
    }

    #[test]
    fn recurrences_round_trip_through_serde() {
        for recurrence in [
            Recurrence::Once {
                at: date(2026, 3, 20).at(10, 0, 0, 0),
            },
            Recurrence::Daily {
                at: time(10, 0, 0, 0),
            },
            Recurrence::Weekly {
                weekday: Weekday::Friday,
                at: time(9, 30, 0, 0),
            },
            Recurrence::Monthly {
                day: 20,
                at: time(10, 0, 0, 0),
            },
        ] {
            let json = serde_json::to_string(&recurrence).unwrap();
            assert_eq!(
                serde_json::from_str::<Recurrence>(&json).unwrap(),
                recurrence
            );
        }
    }

    #[test]
    fn a_reminder_round_trips_through_serde() {
        let mut original = reminder(Recurrence::Monthly {
            day: 20,
            at: time(10, 0, 0, 0),
        });
        original
            .reschedule(at(2026, 3, 5, 9, 0, &tbilisi()))
            .unwrap();
        let json = serde_json::to_string(&original).unwrap();
        assert_eq!(serde_json::from_str::<Reminder>(&json).unwrap(), original);
    }
}
