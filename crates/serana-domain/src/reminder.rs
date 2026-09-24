//! Reminders: what to say, when to say it again, and to whom.
//!
//! Recurrence is modelled as an enum over jiff's date arithmetic rather than as a cron
//! expression. Cron would mean a second time library in the tree, cannot express every
//! pattern a person actually asks for, and is lossy in the direction that matters most
//! here: a stored `0 10 20 * *` has to be translated back into words before it can be
//! shown to the user, whereas a [`Recurrence`] renders itself.

use std::collections::BTreeSet;
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
///
/// Ordered so a set of them renders and stores in calendar order rather than in whatever
/// order the model happened to list them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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

impl From<jiff::civil::Weekday> for Weekday {
    fn from(day: jiff::civil::Weekday) -> Self {
        match day {
            jiff::civil::Weekday::Monday => Self::Monday,
            jiff::civil::Weekday::Tuesday => Self::Tuesday,
            jiff::civil::Weekday::Wednesday => Self::Wednesday,
            jiff::civil::Weekday::Thursday => Self::Thursday,
            jiff::civil::Weekday::Friday => Self::Friday,
            jiff::civil::Weekday::Saturday => Self::Saturday,
            jiff::civil::Weekday::Sunday => Self::Sunday,
        }
    }
}

/// Why a set of days was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidDays {
    #[error("a schedule needs at least one day")]
    Empty,
    #[error("{0} is not a day of the month; days run from 1 to 31")]
    OutOfRange(i8),
}

/// A non-empty set of days of the month, each 1-31.
///
/// A set rather than a single day because real requests are ranges: "every day from the
/// 20th to the 26th" is one reminder with seven firing days, not seven reminders. Stored
/// sorted and deduplicated, so two requests that mean the same schedule compare equal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<i8>", into = "Vec<i8>")]
pub struct MonthDays(BTreeSet<i8>);

impl MonthDays {
    pub fn new(days: impl IntoIterator<Item = i8>) -> Result<Self, InvalidDays> {
        let days: BTreeSet<i8> = days.into_iter().collect();
        if days.is_empty() {
            return Err(InvalidDays::Empty);
        }
        if let Some(&bad) = days.iter().find(|&&day| !(1..=31).contains(&day)) {
            return Err(InvalidDays::OutOfRange(bad));
        }
        Ok(Self(days))
    }

    /// Every day, ascending.
    pub fn iter(&self) -> impl Iterator<Item = i8> + '_ {
        self.0.iter().copied()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Always false — the type cannot be constructed empty. Present because clippy asks for
    /// it next to `len`, and because a caller should not have to know that.
    pub fn is_empty(&self) -> bool {
        false
    }
}

impl TryFrom<Vec<i8>> for MonthDays {
    type Error = InvalidDays;

    fn try_from(days: Vec<i8>) -> Result<Self, Self::Error> {
        Self::new(days)
    }
}

impl From<MonthDays> for Vec<i8> {
    fn from(days: MonthDays) -> Self {
        days.0.into_iter().collect()
    }
}

/// A non-empty set of weekdays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<Weekday>", into = "Vec<Weekday>")]
pub struct WeekDays(BTreeSet<Weekday>);

impl WeekDays {
    pub fn new(days: impl IntoIterator<Item = Weekday>) -> Result<Self, InvalidDays> {
        let days: BTreeSet<Weekday> = days.into_iter().collect();
        if days.is_empty() {
            return Err(InvalidDays::Empty);
        }
        Ok(Self(days))
    }

    pub fn contains(&self, day: jiff::civil::Weekday) -> bool {
        self.0.contains(&Weekday::from(day))
    }

    /// Every weekday, Monday first.
    pub fn iter(&self) -> impl Iterator<Item = Weekday> + '_ {
        self.0.iter().copied()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Always false; see [`MonthDays::is_empty`].
    pub fn is_empty(&self) -> bool {
        false
    }
}

impl TryFrom<Vec<Weekday>> for WeekDays {
    type Error = InvalidDays;

    fn try_from(days: Vec<Weekday>) -> Result<Self, Self::Error> {
        Self::new(days)
    }
}

impl From<WeekDays> for Vec<Weekday> {
    fn from(days: WeekDays) -> Self {
        days.0.into_iter().collect()
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
    /// Fires on each listed weekday, every week.
    Weekly {
        days: WeekDays,
        at: jiff::civil::Time,
    },
    /// Fires on each listed day of the month. Months without a listed day **skip** it
    /// rather than clamping: a reminder for the 31st does not fire on 28 February.
    /// Clamping would fire it on a day the user did not ask for, which is the worse
    /// failure for something like an invoice deadline.
    Monthly {
        days: MonthDays,
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
            Self::Weekly { days, at } => (0..=7).find_map(|offset| {
                let date = today.checked_add(jiff::Span::new().days(offset)).ok()?;
                days.contains(date.weekday())
                    .then(|| candidate_after(date, *at, tz, after))
                    .flatten()
            }),
            Self::Monthly { days, at } => {
                let first = today.first_of_month();
                (0..MAX_MONTHS_SEARCHED).find_map(|offset| {
                    let month = first
                        .checked_add(jiff::Span::new().months(offset as i64))
                        .ok()?;
                    // Days are ascending, so the first candidate in a month is the
                    // earliest one. A day this month does not have is skipped, not
                    // clamped, and the remaining days still get their chance.
                    days.iter().find_map(|day| {
                        if day > month.days_in_month() {
                            return None;
                        }
                        let date = month.with().day(day).build().ok()?;
                        candidate_after(date, *at, tz, after)
                    })
                })
            }
        }
    }

    /// The last instant of the period containing `instant`, or `None` for a recurrence
    /// that has no next period.
    ///
    /// A period is the span a schedule repeats over: a day for [`Self::Daily`], a week
    /// (Monday to Sunday) for [`Self::Weekly`], a calendar month for [`Self::Monthly`].
    /// Acknowledging a reminder suppresses the rest of its current period and nothing
    /// beyond it — "I have sent the invoice, stop asking until next month".
    ///
    /// [`Self::Once`] returns `None`: it has no next period, so acknowledging one retires
    /// it outright.
    pub fn period_end_after(
        &self,
        instant: jiff::Timestamp,
        tz: &jiff::tz::TimeZone,
    ) -> Option<jiff::Timestamp> {
        let date = instant.to_zoned(tz.clone()).date();
        let next_period_starts = match self {
            Self::Once { .. } => return None,
            Self::Daily { .. } => date.checked_add(jiff::Span::new().days(1)).ok()?,
            Self::Weekly { .. } => {
                // `to_monday_zero_offset` is 0 on Monday, so a Monday advances a full week
                // rather than standing still.
                let ahead = 7 - i64::from(date.weekday().to_monday_zero_offset());
                date.checked_add(jiff::Span::new().days(ahead)).ok()?
            }
            Self::Monthly { .. } => date
                .first_of_month()
                .checked_add(jiff::Span::new().months(1))
                .ok()?,
        };
        let starts = to_instant(
            next_period_starts.to_datetime(jiff::civil::time(0, 0, 0, 0)),
            tz,
        )?;
        // One nanosecond before the next period, so an occurrence falling exactly on the
        // period boundary belongs to the new period and still fires.
        starts.checked_sub(jiff::Span::new().nanoseconds(1)).ok()
    }

    /// The first instant of the period containing `instant`, or `None` for a recurrence
    /// with no periods.
    ///
    /// The mirror of [`Self::period_end_after`]. A checklist tick counts only if it landed
    /// at or after this moment, which is what makes the list come back empty next period.
    pub fn period_start_of(
        &self,
        instant: jiff::Timestamp,
        tz: &jiff::tz::TimeZone,
    ) -> Option<jiff::Timestamp> {
        let date = instant.to_zoned(tz.clone()).date();
        let starts = match self {
            Self::Once { .. } => return None,
            Self::Daily { .. } => date,
            Self::Weekly { .. } => {
                let back = i64::from(date.weekday().to_monday_zero_offset());
                date.checked_sub(jiff::Span::new().days(back)).ok()?
            }
            Self::Monthly { .. } => date.first_of_month(),
        };
        to_instant(starts.to_datetime(jiff::civil::time(0, 0, 0, 0)), tz)
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

/// One line of a reminder's checklist.
///
/// `done_at` is a moment rather than a flag, because a recurring checklist has to come back
/// unticked next period. Whether it counts as done is therefore a question about *when* it
/// was ticked, answered by [`Reminder::is_done`] — nothing ever has to reset it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoItem {
    pub text: String,
    #[serde(default)]
    pub done_at: Option<jiff::Timestamp>,
}

impl TodoItem {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            done_at: None,
        }
    }
}

/// A scheduled reminder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reminder {
    pub id: ReminderId,
    pub owner: UserId,
    /// What to send. Stored as the user's own words.
    pub text: String,
    /// The checklist, if there is one. Empty is the ordinary case: a reminder that is just
    /// a message.
    #[serde(default)]
    pub items: Vec<TodoItem>,
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
    /// The end of the period the user has already dealt with, if any.
    ///
    /// Occurrences at or before it are suppressed, which is what makes "done" mean "stop
    /// asking until next month" rather than "delete this". It needs no clearing: once the
    /// next period begins the watermark is in the past and stops having any effect.
    #[serde(default)]
    pub acknowledged_through: Option<jiff::Timestamp>,
}

impl Reminder {
    /// Recompute [`Reminder::next_fire_at`] from `now`.
    ///
    /// Called on creation and after every delivery. Returns the new value. An outstanding
    /// acknowledgement holds the search past the end of its period, so a delivery that
    /// races an acknowledgement cannot resurrect the occurrences it silenced.
    pub fn reschedule(
        &mut self,
        now: jiff::Timestamp,
    ) -> Result<Option<jiff::Timestamp>, StorageError> {
        let tz = self.timezone.resolve()?;
        let floor = self
            .acknowledged_through
            .map_or(now, |through| now.max(through));
        self.next_fire_at = self.recurrence.next_occurrence_after(floor, &tz);
        Ok(self.next_fire_at)
    }

    /// The start of the period `now` falls in, or `None` if the recurrence has no periods.
    fn period_start(&self, now: jiff::Timestamp) -> Result<Option<jiff::Timestamp>, StorageError> {
        let tz = self.timezone.resolve()?;
        Ok(self.recurrence.period_start_of(now, &tz))
    }

    /// Whether `item` counts as ticked for the period `now` falls in.
    ///
    /// A tick from a previous period does not count — that is the whole reason `done_at` is
    /// a timestamp. A one-off has a single period stretching forever, so any tick counts.
    pub fn is_done(&self, item: &TodoItem, now: jiff::Timestamp) -> Result<bool, StorageError> {
        let Some(done_at) = item.done_at else {
            return Ok(false);
        };
        Ok(match self.period_start(now)? {
            Some(start) => done_at >= start,
            None => true,
        })
    }

    /// The items still outstanding this period, in order.
    pub fn outstanding(&self, now: jiff::Timestamp) -> Result<Vec<&TodoItem>, StorageError> {
        let mut left = Vec::new();
        for item in &self.items {
            if !self.is_done(item, now)? {
                left.push(item);
            }
        }
        Ok(left)
    }

    /// Whether every item has been ticked this period. False when there is no checklist —
    /// an empty list is not "finished", it is a reminder that never had one.
    pub fn all_done(&self, now: jiff::Timestamp) -> Result<bool, StorageError> {
        if self.items.is_empty() {
            return Ok(false);
        }
        Ok(self.outstanding(now)?.is_empty())
    }

    /// Tick the items matching `wanted`, returning the text of each one actually ticked.
    ///
    /// Matching is forgiving because the phrases come from a model repeating the user back:
    /// an exact match first, then a single unambiguous containment. An ambiguous phrase
    /// ticks nothing, because ticking the wrong line is worse than ticking none.
    pub fn complete(&mut self, wanted: &[String], now: jiff::Timestamp) -> Vec<String> {
        let mut ticked = Vec::new();
        for phrase in wanted {
            let needle = phrase.trim().to_lowercase();
            if needle.is_empty() {
                continue;
            }
            let exact: Vec<usize> = self
                .items
                .iter()
                .enumerate()
                .filter(|(_, item)| item.text.trim().to_lowercase() == needle)
                .map(|(index, _)| index)
                .collect();
            let candidates = if exact.is_empty() {
                self.items
                    .iter()
                    .enumerate()
                    .filter(|(_, item)| {
                        let text = item.text.to_lowercase();
                        text.contains(&needle) || needle.contains(&text)
                    })
                    .map(|(index, _)| index)
                    .collect()
            } else {
                exact
            };

            if let [only] = candidates[..] {
                self.items[only].done_at = Some(now);
                ticked.push(self.items[only].text.clone());
            }
        }
        ticked
    }

    /// Replace the checklist, keeping the ticks of any line that survives the edit.
    ///
    /// Adding a fourth thing to do should not un-tick the three already done. Lines are
    /// matched by their text, which is the only identity a checklist item has — rename one
    /// and it is a new line, which is the honest reading of a rename.
    pub fn relist(&mut self, items: Vec<TodoItem>) {
        let previous = std::mem::take(&mut self.items);
        self.items = items
            .into_iter()
            .map(|mut item| {
                if item.done_at.is_none() {
                    let needle = item.text.trim().to_lowercase();
                    item.done_at = previous
                        .iter()
                        .find(|old| old.text.trim().to_lowercase() == needle)
                        .and_then(|old| old.done_at);
                }
                item
            })
            .collect();
    }

    /// Untick everything, so the checklist reads as fresh for the period.
    pub fn reopen(&mut self) {
        for item in &mut self.items {
            item.done_at = None;
        }
    }

    /// Record that the user has dealt with this period, and skip to the next one.
    ///
    /// Returns the new firing time: `None` means nothing further is scheduled, which for a
    /// [`Recurrence::Once`] is the normal outcome.
    pub fn acknowledge(
        &mut self,
        now: jiff::Timestamp,
    ) -> Result<Option<jiff::Timestamp>, StorageError> {
        let tz = self.timezone.resolve()?;
        self.acknowledged_through = self.recurrence.period_end_after(now, &tz);
        match self.acknowledged_through {
            Some(_) => self.reschedule(now),
            // A one-off has no next period: acknowledging it is the end of it.
            None => {
                self.next_fire_at = None;
                Ok(None)
            }
        }
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
    /// Deliver `reminder` to `owner`.
    ///
    /// Takes the whole reminder rather than a rendered string because what arrives depends
    /// on it: a checklist shows what is still outstanding, a plain reminder shows its text.
    /// Composing that sentence is the frontend's job, and the frontend is the only layer
    /// that may read `serana-app`.
    /// `now` decides which checklist items count as outstanding. It is passed in rather
    /// than read from a clock here so that the whole tick — what is due, what is still
    /// undone, what is recorded — is reasoned about at one instant.
    async fn notify(
        &self,
        owner: UserId,
        reminder: &Reminder,
        now: jiff::Timestamp,
    ) -> Result<(), NotifyError>;
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
    async fn notify(
        &self,
        owner: UserId,
        reminder: &Reminder,
        now: jiff::Timestamp,
    ) -> Result<(), NotifyError> {
        (**self).notify(owner, reminder, now).await
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
            days: WeekDays::new([Weekday::Friday]).unwrap(),
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
            days: WeekDays::new([Weekday::Tuesday]).unwrap(),
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
            days: WeekDays::new([Weekday::Tuesday]).unwrap(),
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
            days: MonthDays::new([20]).unwrap(),
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
            days: MonthDays::new([31]).unwrap(),
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
            days: MonthDays::new([29]).unwrap(),
            at: time(8, 0, 0, 0),
        };
        // 2028 is a leap year.
        let next = r
            .next_occurrence_after(at(2028, 2, 1, 0, 0, &tz), &tz)
            .unwrap();
        assert_eq!(local(next, &tz), date(2028, 2, 29).at(8, 0, 0, 0));
    }

    #[test]
    fn an_impossible_day_of_the_month_cannot_be_constructed() {
        // This used to be a schedule that silently never fired. Refusing it at the
        // constructor means a hallucinated day 45 is reported to the user instead.
        for day in [0, 32, -1, 100] {
            assert_eq!(
                MonthDays::new([day]),
                Err(InvalidDays::OutOfRange(day)),
                "day {day}"
            );
        }
        assert_eq!(MonthDays::new([]), Err(InvalidDays::Empty));
        assert_eq!(WeekDays::new([]), Err(InvalidDays::Empty));
    }

    /// A reminder nagging daily from the 20th to the 26th, in Tbilisi.
    fn nagging() -> Reminder {
        Reminder {
            id: ReminderId::new("r1"),
            owner: UserId::new(1),
            text: "issue the invoice".into(),
            items: Vec::new(),
            recurrence: Recurrence::Monthly {
                days: MonthDays::new([20, 21, 22, 23, 24, 25, 26]).unwrap(),
                at: time(22, 30, 0, 0),
            },
            timezone: TimeZoneName::new("Asia/Tbilisi"),
            created_at: jiff::Timestamp::UNIX_EPOCH,
            next_fire_at: None,
            last_fired_at: None,
            acknowledged_through: None,
        }
    }

    /// The payment checklist from the brief: days 1-6 each month, three things to do.
    fn payment_checklist() -> Reminder {
        let mut r = nagging();
        r.text = "monthly payment".into();
        r.recurrence = Recurrence::Monthly {
            days: MonthDays::new([1, 2, 3, 4, 5, 6]).unwrap(),
            at: time(9, 0, 0, 0),
        };
        r.items = vec![
            TodoItem::new("exchange money"),
            TodoItem::new("transfer to tbc"),
            TodoItem::new("write to the banker"),
        ];
        r
    }

    #[test]
    fn a_tick_counts_this_period_and_goes_stale_in_the_next() {
        // The whole reason `done_at` is a timestamp rather than a flag.
        let tz = tbilisi();
        let mut r = payment_checklist();
        let march = at(2026, 3, 2, 10, 0, &tz);
        r.complete(&["exchange money".into()], march);

        assert!(r.is_done(&r.items[0], march).unwrap(), "done in March");
        assert_eq!(r.outstanding(march).unwrap().len(), 2);

        // April: the same tick no longer counts, and nothing had to reset it.
        let april = at(2026, 4, 1, 10, 0, &tz);
        assert!(!r.is_done(&r.items[0], april).unwrap());
        assert_eq!(r.outstanding(april).unwrap().len(), 3);
    }

    #[test]
    fn a_tick_earlier_in_the_same_period_still_counts() {
        // Day 1 ticked, read back on day 6: the window is the month, not the day.
        let tz = tbilisi();
        let mut r = payment_checklist();
        r.complete(&["exchange money".into()], at(2026, 3, 1, 10, 0, &tz));
        assert!(
            r.is_done(&r.items[0], at(2026, 3, 6, 23, 0, &tz)).unwrap(),
            "still done on the 6th"
        );
    }

    #[test]
    fn matching_is_forgiving_but_refuses_to_guess_between_two_lines() {
        let tz = tbilisi();
        let now = at(2026, 3, 2, 10, 0, &tz);
        let mut r = payment_checklist();

        // Exact, and a paraphrase that contains only one line.
        assert_eq!(
            r.complete(&["EXCHANGE MONEY".into(), "transfer".into()], now),
            vec!["exchange money", "transfer to tbc"]
        );

        // "to" appears in two remaining lines... and in none now, so nothing is ticked.
        let mut fresh = payment_checklist();
        assert!(fresh.complete(&["to".into()], now).is_empty(), "ambiguous");
        assert_eq!(fresh.outstanding(now).unwrap().len(), 3);
    }

    #[test]
    fn a_phrase_matching_nothing_ticks_nothing() {
        let tz = tbilisi();
        let now = at(2026, 3, 2, 10, 0, &tz);
        let mut r = payment_checklist();
        assert!(
            r.complete(&["feed the cat".into(), "  ".into()], now)
                .is_empty()
        );
        assert_eq!(r.outstanding(now).unwrap().len(), 3);
    }

    #[test]
    fn all_done_is_false_for_a_reminder_with_no_checklist() {
        // An empty list is not "finished" — it never had anything to finish.
        let tz = tbilisi();
        assert!(!nagging().all_done(at(2026, 3, 20, 23, 0, &tz)).unwrap());
    }

    #[test]
    fn finishing_the_checklist_is_what_ends_the_period() {
        let tz = tbilisi();
        let mut r = payment_checklist();
        r.reschedule(at(2026, 3, 1, 0, 0, &tz)).unwrap();

        let now = at(2026, 3, 2, 10, 0, &tz);
        r.complete(
            &[
                "exchange money".into(),
                "transfer to tbc".into(),
                "write to the banker".into(),
            ],
            now,
        );
        assert!(r.all_done(now).unwrap());

        // Acknowledging on the strength of that skips days 3-6.
        r.acknowledge(now).unwrap();
        assert_eq!(
            local(r.next_fire_at.unwrap(), &tz),
            date(2026, 4, 1).at(9, 0, 0, 0)
        );
    }

    #[test]
    fn a_period_start_is_the_mirror_of_its_end() {
        let tz = tbilisi();
        let noon = at(2026, 3, 11, 12, 0, &tz);
        let start = |r: &Recurrence| local(r.period_start_of(noon, &tz).unwrap(), &tz).date();

        // 2026-03-11 is a Wednesday, so its week began on Monday the 9th.
        assert_eq!(
            start(&Recurrence::Daily {
                at: time(9, 0, 0, 0)
            }),
            date(2026, 3, 11)
        );
        assert_eq!(
            start(&Recurrence::Weekly {
                days: WeekDays::new([Weekday::Monday]).unwrap(),
                at: time(9, 0, 0, 0)
            }),
            date(2026, 3, 9)
        );
        assert_eq!(start(&payment_checklist().recurrence), date(2026, 3, 1));
        assert_eq!(
            Recurrence::Once {
                at: date(2026, 3, 20).at(9, 0, 0, 0)
            }
            .period_start_of(noon, &tz),
            None,
            "a one-off has no periods, so a tick never goes stale"
        );
    }

    #[test]
    fn acknowledging_silences_the_rest_of_the_period_and_no_further() {
        let tz = tbilisi();
        let mut r = nagging();
        r.reschedule(at(2026, 3, 1, 0, 0, &tz)).unwrap();
        assert_eq!(
            local(r.next_fire_at.unwrap(), &tz),
            date(2026, 3, 20).at(22, 30, 0, 0)
        );

        // The user replies "done" on the 21st, having sent the invoice.
        let next = r.acknowledge(at(2026, 3, 21, 23, 0, &tz)).unwrap().unwrap();
        // The 22nd through the 26th are skipped; April starts clean.
        assert_eq!(local(next, &tz), date(2026, 4, 20).at(22, 30, 0, 0));
    }

    #[test]
    fn an_acknowledgement_expires_with_its_period_rather_than_needing_to_be_cleared() {
        let tz = tbilisi();
        let mut r = nagging();
        r.acknowledge(at(2026, 3, 21, 23, 0, &tz)).unwrap();
        let watermark = r.acknowledged_through.unwrap();

        // Once April is under way the stale watermark has no effect at all.
        r.reschedule(at(2026, 4, 22, 0, 0, &tz)).unwrap();
        assert!(watermark < at(2026, 4, 22, 0, 0, &tz));
        assert_eq!(
            local(r.next_fire_at.unwrap(), &tz),
            date(2026, 4, 22).at(22, 30, 0, 0)
        );
    }

    #[test]
    fn a_delivery_racing_an_acknowledgement_cannot_revive_the_silenced_days() {
        // The scheduler may be mid-tick when the user answers. `mark_fired` must not undo
        // the acknowledgement by rescheduling from `now`.
        let tz = tbilisi();
        let mut r = nagging();
        r.acknowledge(at(2026, 3, 21, 23, 0, &tz)).unwrap();
        r.mark_fired(at(2026, 3, 21, 23, 1, &tz)).unwrap();
        assert_eq!(
            local(r.next_fire_at.unwrap(), &tz),
            date(2026, 4, 20).at(22, 30, 0, 0),
            "the 22nd must stay silenced"
        );
    }

    #[test]
    fn acknowledging_a_one_off_retires_it() {
        let tz = tbilisi();
        let mut r = nagging();
        r.recurrence = Recurrence::Once {
            at: date(2026, 3, 20).at(9, 0, 0, 0),
        };
        r.reschedule(at(2026, 3, 1, 0, 0, &tz)).unwrap();
        assert!(r.is_active());

        assert_eq!(r.acknowledge(at(2026, 3, 20, 9, 5, &tz)).unwrap(), None);
        assert!(!r.is_active(), "a one-off has no next period");
    }

    #[test]
    fn a_period_is_the_calendar_unit_the_recurrence_repeats_over() {
        let tz = tbilisi();
        let noon = at(2026, 3, 11, 12, 0, &tz);
        let end = |r: &Recurrence| local(r.period_end_after(noon, &tz).unwrap(), &tz).date();

        // 2026-03-11 is a Wednesday, so its week ends on Sunday the 15th.
        assert_eq!(
            end(&Recurrence::Daily {
                at: time(9, 0, 0, 0)
            }),
            date(2026, 3, 11)
        );
        assert_eq!(
            end(&Recurrence::Weekly {
                days: WeekDays::new([Weekday::Monday]).unwrap(),
                at: time(9, 0, 0, 0)
            }),
            date(2026, 3, 15)
        );
        assert_eq!(end(&nagging().recurrence), date(2026, 3, 31));
        assert_eq!(
            Recurrence::Once {
                at: date(2026, 3, 20).at(9, 0, 0, 0)
            }
            .period_end_after(noon, &tz),
            None
        );
    }

    #[test]
    fn days_are_sorted_and_deduplicated_so_equal_schedules_compare_equal() {
        let scrambled = MonthDays::new([26, 20, 22, 20, 21]).unwrap();
        assert_eq!(scrambled.iter().collect::<Vec<_>>(), vec![20, 21, 22, 26]);
        assert_eq!(scrambled.len(), 4);
        assert_eq!(MonthDays::new([20, 21]), MonthDays::new([21, 20, 21]));
    }

    #[test]
    fn a_monthly_range_fires_on_every_day_of_the_range() {
        // The request that motivated day sets: nag daily from the 20th to the 26th.
        let tz = tbilisi();
        let r = Recurrence::Monthly {
            days: MonthDays::new([20, 21, 22, 23, 24, 25, 26]).unwrap(),
            at: time(22, 30, 0, 0),
        };
        let mut cursor = at(2026, 3, 1, 0, 0, &tz);
        let mut fired = Vec::new();
        for _ in 0..9 {
            cursor = r.next_occurrence_after(cursor, &tz).unwrap();
            fired.push(local(cursor, &tz));
        }
        // Seven days this month, then it rolls into the next.
        assert_eq!(fired[0], date(2026, 3, 20).at(22, 30, 0, 0));
        assert_eq!(fired[6], date(2026, 3, 26).at(22, 30, 0, 0));
        assert_eq!(fired[7], date(2026, 4, 20).at(22, 30, 0, 0));
        assert_eq!(fired[8], date(2026, 4, 21).at(22, 30, 0, 0));
    }

    #[test]
    fn a_month_without_a_listed_day_skips_it_and_keeps_the_others() {
        // February has no 30th or 31st; the 28th still fires.
        let tz = tbilisi();
        let r = Recurrence::Monthly {
            days: MonthDays::new([28, 30, 31]).unwrap(),
            at: time(9, 0, 0, 0),
        };
        let first = r
            .next_occurrence_after(at(2026, 2, 1, 0, 0, &tz), &tz)
            .unwrap();
        assert_eq!(local(first, &tz), date(2026, 2, 28).at(9, 0, 0, 0));
        let second = r.next_occurrence_after(first, &tz).unwrap();
        assert_eq!(local(second, &tz), date(2026, 3, 28).at(9, 0, 0, 0));
    }

    #[test]
    fn a_weekly_reminder_fires_on_each_listed_weekday() {
        let tz = tbilisi();
        let r = Recurrence::Weekly {
            days: WeekDays::new([Weekday::Monday, Weekday::Friday]).unwrap(),
            at: time(18, 0, 0, 0),
        };
        // 2026-03-09 is a Monday.
        let mut cursor = at(2026, 3, 8, 0, 0, &tz);
        let mut fired = Vec::new();
        for _ in 0..3 {
            cursor = r.next_occurrence_after(cursor, &tz).unwrap();
            fired.push(local(cursor, &tz));
        }
        assert_eq!(fired[0], date(2026, 3, 9).at(18, 0, 0, 0));
        assert_eq!(fired[1], date(2026, 3, 13).at(18, 0, 0, 0));
        assert_eq!(fired[2], date(2026, 3, 16).at(18, 0, 0, 0));
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
            items: Vec::new(),
            recurrence,
            timezone: TimeZoneName::new("Asia/Tbilisi"),
            created_at: jiff::Timestamp::UNIX_EPOCH,
            next_fire_at: None,
            last_fired_at: None,
            acknowledged_through: None,
        }
    }

    #[test]
    fn rescheduling_fills_in_the_next_firing_time() {
        let tz = tbilisi();
        let mut r = reminder(Recurrence::Monthly {
            days: MonthDays::new([20]).unwrap(),
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
            days: MonthDays::new([20]).unwrap(),
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
                days: WeekDays::new([Weekday::Friday]).unwrap(),
                at: time(9, 30, 0, 0),
            },
            Recurrence::Monthly {
                days: MonthDays::new([20]).unwrap(),
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
            days: MonthDays::new([20]).unwrap(),
            at: time(10, 0, 0, 0),
        });
        original
            .reschedule(at(2026, 3, 5, 9, 0, &tbilisi()))
            .unwrap();
        let json = serde_json::to_string(&original).unwrap();
        assert_eq!(serde_json::from_str::<Reminder>(&json).unwrap(), original);
    }
}
