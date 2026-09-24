//! When a reminder fires.
//!
//! Recurrence is modelled as an enum over jiff's date arithmetic rather than as a cron
//! expression. Cron would mean a second time library in the tree, cannot express every
//! pattern a person actually asks for, and is lossy in the direction that matters most
//! here: a stored `0 10 20 * *` has to be translated back into words before it can be
//! shown to the user, whereas a [`Recurrence`] renders itself.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

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
