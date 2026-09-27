//! Turning "every month on the 20th" — in whatever language the person wrote it — into a
//! [`Recurrence`].
//!
//! Shared by everything that repeats. A reminder and a price digest fire on the same
//! vocabulary, and a second copy of this is a second place for "the 31st" to be handled
//! differently.
//!
//! The model does the language; this module does the validation. Everything here is a pure
//! function over the model's output, so the rules are tested without a provider — which
//! matters, because this is where a plausible-looking hallucination becomes something that
//! fires at the wrong time, or never.

use schemars::JsonSchema;
use serde::Deserialize;

use serana_domain::reminder::{InvalidDays, MonthDays, Recurrence, WeekDays, Weekday};

/// The shape the model is asked to produce for anything that repeats.
///
/// Flat and stringly-typed where the model is good at it (dates, times) and enumerated
/// where it is not (the kind, the weekday). A nested tagged union would be a truer model of
/// [`Recurrence`], and weaker models fill it in badly.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct ParsedSchedule {
    /// How often it repeats.
    pub kind: ParsedKind,
    /// Time of day on a 24-hour clock, as `HH:MM`.
    pub time: String,
    /// Which days of the week. Required when `kind` is `weekly`, ignored otherwise.
    /// List every day it should fire on.
    #[serde(default)]
    pub weekdays: Option<Vec<ParsedWeekday>>,
    /// Days of the month, each 1 to 31. Required when `kind` is `monthly`, ignored
    /// otherwise. List every day, expanding ranges: "from the 20th to the 26th" is
    /// [20, 21, 22, 23, 24, 25, 26].
    #[serde(default)]
    pub days_of_month: Option<Vec<i8>>,
    /// Calendar date as `YYYY-MM-DD`. Required when `kind` is `once`, ignored otherwise.
    #[serde(default)]
    pub date: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ParsedKind {
    Once,
    Daily,
    Weekly,
    Monthly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ParsedWeekday {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

impl From<ParsedWeekday> for Weekday {
    fn from(day: ParsedWeekday) -> Self {
        match day {
            ParsedWeekday::Monday => Self::Monday,
            ParsedWeekday::Tuesday => Self::Tuesday,
            ParsedWeekday::Wednesday => Self::Wednesday,
            ParsedWeekday::Thursday => Self::Thursday,
            ParsedWeekday::Friday => Self::Friday,
            ParsedWeekday::Saturday => Self::Saturday,
            ParsedWeekday::Sunday => Self::Sunday,
        }
    }
}

impl ParsedSchedule {
    /// Validate and convert.
    ///
    /// The failure is a sentence rather than a typed error: every caller wraps it in its own
    /// "I did not understand" variant and shows it to the person, so a taxonomy here would
    /// be one nobody branches on.
    pub(crate) fn into_recurrence(self) -> Result<Recurrence, String> {
        let time = parse_time(&self.time)?;
        Ok(match self.kind {
            ParsedKind::Daily => Recurrence::Daily { at: time },
            ParsedKind::Weekly => {
                let weekdays = self.weekdays.unwrap_or_default();
                let days = WeekDays::new(weekdays.into_iter().map(Weekday::from))
                    .map_err(|_| "a weekly schedule needs a weekday".to_owned())?;
                Recurrence::Weekly { days, at: time }
            }
            ParsedKind::Monthly => {
                let days_of_month = self.days_of_month.unwrap_or_default();
                // `MonthDays` owns the range check, so a hallucinated day 45 is refused in
                // one place rather than wherever someone remembered to look.
                let days = MonthDays::new(days_of_month).map_err(|error| match error {
                    InvalidDays::Empty => "a monthly schedule needs a day of the month".to_owned(),
                    InvalidDays::OutOfRange(day) => format!("{day} is not a day of the month"),
                })?;
                Recurrence::Monthly { days, at: time }
            }
            ParsedKind::Once => {
                let raw = self
                    .date
                    .ok_or_else(|| "a one-off needs a date".to_owned())?;
                let date: jiff::civil::Date = raw
                    .parse()
                    .map_err(|e| format!("{raw:?} is not a date: {e}"))?;
                Recurrence::Once {
                    at: date.to_datetime(time),
                }
            }
        })
    }
}

/// Accept `HH:MM` and `HH:MM:SS`, with or without a leading zero.
///
/// Lenient because models are inconsistent about zero-padding and sometimes volunteer
/// seconds, and none of that is a reason to refuse.
pub(crate) fn parse_time(raw: &str) -> Result<jiff::civil::Time, String> {
    let unusable = || format!("{raw:?} is not a time of day");
    let mut parts = raw.trim().split(':');
    let hour: i8 = parts
        .next()
        .ok_or_else(unusable)?
        .trim()
        .parse()
        .map_err(|_| unusable())?;
    let minute: i8 = parts
        .next()
        .ok_or_else(unusable)?
        .trim()
        .parse()
        .map_err(|_| unusable())?;
    let second: i8 = match parts.next() {
        Some(s) => s.trim().parse().map_err(|_| unusable())?,
        None => 0,
    };
    if parts.next().is_some() {
        return Err(unusable());
    }
    jiff::civil::Time::new(hour, minute, second, 0).map_err(|_| unusable())
}
