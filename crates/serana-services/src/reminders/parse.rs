//! Turning "каждый месяц 20 число" into a [`Recurrence`].
//!
//! The model does the language; this module does the validation. Everything here is a pure
//! function over the model's output, so the rules are tested without a provider — which
//! matters, because this is where a plausible-looking hallucination becomes a reminder that
//! fires at the wrong time, or never.

use schemars::JsonSchema;
use serde::Deserialize;

use serana_domain::reminder::{Recurrence, Weekday};

use super::ReminderError;

/// The shape the model is asked to produce.
///
/// Flat and stringly-typed where the model is good at it (dates, times) and enumerated
/// where it is not (the kind, the weekday). A nested tagged union would be a truer model of
/// [`Recurrence`], and weaker models fill it in badly.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct ParsedReminder {
    /// How often the reminder repeats.
    pub kind: ParsedKind,
    /// Time of day on a 24-hour clock, as `HH:MM`.
    pub time: String,
    /// Which day of the week. Required when `kind` is `weekly`, ignored otherwise.
    #[serde(default)]
    pub weekday: Option<ParsedWeekday>,
    /// Day of the month, 1 to 31. Required when `kind` is `monthly`, ignored otherwise.
    #[serde(default)]
    pub day_of_month: Option<i8>,
    /// Calendar date as `YYYY-MM-DD`. Required when `kind` is `once`, ignored otherwise.
    #[serde(default)]
    pub date: Option<String>,
    /// What to send when the reminder fires, in the user's own words.
    pub text: String,
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

impl ParsedReminder {
    /// Validate and convert. The returned `String` is the reminder's own text.
    pub(crate) fn into_recurrence(self) -> Result<(Recurrence, String), ReminderError> {
        let time = parse_time(&self.time)?;
        let text = self.text.trim().to_owned();
        if text.is_empty() {
            return Err(ReminderError::Unparsable("the reminder has no text".into()));
        }

        let recurrence = match self.kind {
            ParsedKind::Daily => Recurrence::Daily { at: time },
            ParsedKind::Weekly => {
                let weekday = self.weekday.ok_or_else(|| {
                    ReminderError::Unparsable("a weekly reminder needs a weekday".into())
                })?;
                Recurrence::Weekly {
                    weekday: weekday.into(),
                    at: time,
                }
            }
            ParsedKind::Monthly => {
                let day = self.day_of_month.ok_or_else(|| {
                    ReminderError::Unparsable("a monthly reminder needs a day of the month".into())
                })?;
                if !(1..=31).contains(&day) {
                    return Err(ReminderError::Unparsable(format!(
                        "{day} is not a day of the month"
                    )));
                }
                Recurrence::Monthly { day, at: time }
            }
            ParsedKind::Once => {
                let raw = self.date.ok_or_else(|| {
                    ReminderError::Unparsable("a one-off reminder needs a date".into())
                })?;
                let date: jiff::civil::Date = raw.parse().map_err(|e| {
                    ReminderError::Unparsable(format!("{raw:?} is not a date: {e}"))
                })?;
                Recurrence::Once {
                    at: date.to_datetime(time),
                }
            }
        };

        Ok((recurrence, text))
    }
}

/// Accept `HH:MM` and `HH:MM:SS`, with or without a leading zero.
///
/// Lenient because models are inconsistent about zero-padding and sometimes volunteer
/// seconds, and none of that is a reason to refuse a reminder.
fn parse_time(raw: &str) -> Result<jiff::civil::Time, ReminderError> {
    let unusable = || ReminderError::Unparsable(format!("{raw:?} is not a time of day"));
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

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::time;

    fn parsed(kind: ParsedKind) -> ParsedReminder {
        ParsedReminder {
            kind,
            time: "10:00".into(),
            weekday: None,
            day_of_month: None,
            date: None,
            text: "оформить invoice".into(),
        }
    }

    #[test]
    fn the_brief_example_becomes_a_monthly_recurrence() {
        let mut p = parsed(ParsedKind::Monthly);
        p.day_of_month = Some(20);
        let (recurrence, text) = p.into_recurrence().unwrap();
        assert_eq!(
            recurrence,
            Recurrence::Monthly {
                day: 20,
                at: time(10, 0, 0, 0)
            }
        );
        assert_eq!(text, "оформить invoice");
    }

    #[test]
    fn a_daily_reminder_needs_nothing_but_a_time() {
        let (recurrence, _) = parsed(ParsedKind::Daily).into_recurrence().unwrap();
        assert_eq!(
            recurrence,
            Recurrence::Daily {
                at: time(10, 0, 0, 0)
            }
        );
    }

    #[test]
    fn a_weekly_reminder_carries_its_weekday() {
        let mut p = parsed(ParsedKind::Weekly);
        p.weekday = Some(ParsedWeekday::Friday);
        let (recurrence, _) = p.into_recurrence().unwrap();
        assert_eq!(
            recurrence,
            Recurrence::Weekly {
                weekday: Weekday::Friday,
                at: time(10, 0, 0, 0)
            }
        );
    }

    #[test]
    fn a_one_off_reminder_combines_its_date_and_time() {
        let mut p = parsed(ParsedKind::Once);
        p.date = Some("2026-03-20".into());
        let (recurrence, _) = p.into_recurrence().unwrap();
        assert_eq!(
            recurrence,
            Recurrence::Once {
                at: jiff::civil::date(2026, 3, 20).at(10, 0, 0, 0)
            }
        );
    }

    #[test]
    fn a_kind_missing_its_required_field_is_refused_with_a_reason() {
        // The model claimed "weekly" and then did not say which day. Guessing Monday would
        // produce a reminder that silently fires on the wrong day every week.
        for (kind, expected) in [
            (ParsedKind::Weekly, "weekday"),
            (ParsedKind::Monthly, "day of the month"),
            (ParsedKind::Once, "date"),
        ] {
            let err = parsed(kind).into_recurrence().unwrap_err();
            let message = err.to_string();
            assert!(
                message.contains(expected),
                "{kind:?} should complain about {expected}: {message}"
            );
        }
    }

    #[test]
    fn an_impossible_day_of_the_month_is_refused() {
        for day in [0, 32, -1, 99] {
            let mut p = parsed(ParsedKind::Monthly);
            p.day_of_month = Some(day);
            assert!(p.into_recurrence().is_err(), "{day} should be refused");
        }
    }

    #[test]
    fn valid_days_of_the_month_are_accepted_at_the_boundaries() {
        for day in [1, 28, 31] {
            let mut p = parsed(ParsedKind::Monthly);
            p.day_of_month = Some(day);
            assert!(p.into_recurrence().is_ok(), "{day} should be accepted");
        }
    }

    #[test]
    fn times_are_read_leniently_because_models_format_them_inconsistently() {
        for (raw, expected) in [
            ("10:00", time(10, 0, 0, 0)),
            ("9:05", time(9, 5, 0, 0)),
            ("09:05", time(9, 5, 0, 0)),
            ("23:59", time(23, 59, 0, 0)),
            ("00:00", time(0, 0, 0, 0)),
            ("10:00:30", time(10, 0, 30, 0)),
            (" 10:00 ", time(10, 0, 0, 0)),
        ] {
            assert_eq!(parse_time(raw).unwrap(), expected, "{raw}");
        }
    }

    #[test]
    fn a_time_outside_the_clock_is_refused_rather_than_wrapped() {
        for raw in [
            "24:00",
            "10:60",
            "-1:00",
            "10",
            "10:00:00:00",
            "ten o'clock",
            "",
        ] {
            assert!(parse_time(raw).is_err(), "{raw:?} should be refused");
        }
    }

    #[test]
    fn a_malformed_date_is_refused() {
        let mut p = parsed(ParsedKind::Once);
        p.date = Some("20 марта".into());
        assert!(p.into_recurrence().is_err());
    }

    #[test]
    fn a_reminder_with_no_text_is_refused() {
        for text in ["", "   ", "\n"] {
            let mut p = parsed(ParsedKind::Daily);
            p.text = text.into();
            assert!(p.into_recurrence().is_err(), "{text:?} should be refused");
        }
    }

    #[test]
    fn surrounding_whitespace_is_stripped_from_the_text() {
        let mut p = parsed(ParsedKind::Daily);
        p.text = "  оформить invoice \n".into();
        assert_eq!(p.into_recurrence().unwrap().1, "оформить invoice");
    }

    #[test]
    fn fields_irrelevant_to_the_kind_are_ignored_rather_than_rejected() {
        // Models volunteer extra fields; that is not a reason to refuse the reminder.
        let mut p = parsed(ParsedKind::Daily);
        p.weekday = Some(ParsedWeekday::Friday);
        p.day_of_month = Some(20);
        p.date = Some("2026-03-20".into());
        assert_eq!(
            p.into_recurrence().unwrap().0,
            Recurrence::Daily {
                at: time(10, 0, 0, 0)
            }
        );
    }
}
