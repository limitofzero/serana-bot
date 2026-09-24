//! Turning "every month on the 20th" — in whatever language the user wrote it — into a
//! [`Recurrence`].
//!
//! The model does the language; this module does the validation. Everything here is a pure
//! function over the model's output, so the rules are tested without a provider — which
//! matters, because this is where a plausible-looking hallucination becomes a reminder that
//! fires at the wrong time, or never.

use schemars::JsonSchema;
use serde::Deserialize;

use serana_domain::reminder::{InvalidDays, MonthDays, Recurrence, TodoItem, WeekDays, Weekday};

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
    /// Which days of the week. Required when `kind` is `weekly`, ignored otherwise.
    /// List every day the reminder should fire on.
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
    /// A SHORT heading, a few words, in the person's own language. Never a list: if what
    /// they want is several separate things, those go in `items` and this becomes a name
    /// for the group, such as "monthly payment" or "moving day". A comma-separated run of
    /// tasks here is always wrong.
    pub text: String,
    /// One entry per thing to do, each tickable on its own.
    ///
    /// Use this whenever what the person wants is more than one action — anything joined
    /// by commas, by "and", or written as separate lines. "exchange money, transfer to
    /// tbc, write to the banker" is three entries, never one heading. Leave it out only
    /// when there is genuinely a single thing to do.
    #[serde(default)]
    pub items: Option<Vec<String>>,
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
    pub(crate) fn into_recurrence(
        self,
    ) -> Result<(Recurrence, String, Vec<TodoItem>), ReminderError> {
        let time = parse_time(&self.time)?;
        let text = self.text.trim().to_owned();
        // Blank lines would render as empty checkboxes nobody can ever tick.
        let items: Vec<TodoItem> = self
            .items
            .unwrap_or_default()
            .into_iter()
            .map(|line| line.trim().to_owned())
            .filter(|line| !line.is_empty())
            .map(TodoItem::new)
            .collect();
        if text.is_empty() {
            return Err(ReminderError::Unparsable("the reminder has no text".into()));
        }

        let recurrence = match self.kind {
            ParsedKind::Daily => Recurrence::Daily { at: time },
            ParsedKind::Weekly => {
                let weekdays = self.weekdays.unwrap_or_default();
                let days =
                    WeekDays::new(weekdays.into_iter().map(Weekday::from)).map_err(|_| {
                        ReminderError::Unparsable("a weekly reminder needs a weekday".into())
                    })?;
                Recurrence::Weekly { days, at: time }
            }
            ParsedKind::Monthly => {
                let days_of_month = self.days_of_month.unwrap_or_default();
                // `MonthDays` owns the range check, so a hallucinated day 45 is refused in
                // one place rather than wherever someone remembered to look.
                let days = MonthDays::new(days_of_month).map_err(|error| match error {
                    InvalidDays::Empty => ReminderError::Unparsable(
                        "a monthly reminder needs a day of the month".into(),
                    ),
                    InvalidDays::OutOfRange(day) => {
                        ReminderError::Unparsable(format!("{day} is not a day of the month"))
                    }
                })?;
                Recurrence::Monthly { days, at: time }
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

        Ok((recurrence, text, items))
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
            weekdays: None,
            days_of_month: None,
            items: None,
            date: None,
            text: "оформить invoice".into(),
        }
    }

    #[test]
    fn the_brief_example_becomes_a_monthly_recurrence() {
        let mut p = parsed(ParsedKind::Monthly);
        p.days_of_month = Some(vec![20]);
        let (recurrence, text, _items) = p.into_recurrence().unwrap();
        assert_eq!(
            recurrence,
            Recurrence::Monthly {
                days: MonthDays::new([20]).unwrap(),
                at: time(10, 0, 0, 0)
            }
        );
        assert_eq!(text, "оформить invoice");
    }

    #[test]
    fn a_daily_reminder_needs_nothing_but_a_time() {
        let (recurrence, _, _items) = parsed(ParsedKind::Daily).into_recurrence().unwrap();
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
        p.weekdays = Some(vec![ParsedWeekday::Friday]);
        let (recurrence, _, _items) = p.into_recurrence().unwrap();
        assert_eq!(
            recurrence,
            Recurrence::Weekly {
                days: WeekDays::new([Weekday::Friday]).unwrap(),
                at: time(10, 0, 0, 0)
            }
        );
    }

    #[test]
    fn a_one_off_reminder_combines_its_date_and_time() {
        let mut p = parsed(ParsedKind::Once);
        p.date = Some("2026-03-20".into());
        let (recurrence, _, _items) = p.into_recurrence().unwrap();
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
            p.days_of_month = Some(vec![day]);
            assert!(p.into_recurrence().is_err(), "{day} should be refused");
        }
    }

    #[test]
    fn valid_days_of_the_month_are_accepted_at_the_boundaries() {
        for day in [1, 28, 31] {
            let mut p = parsed(ParsedKind::Monthly);
            p.days_of_month = Some(vec![day]);
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
        p.weekdays = Some(vec![ParsedWeekday::Friday]);
        p.days_of_month = Some(vec![20]);
        p.date = Some("2026-03-20".into());
        assert_eq!(
            p.into_recurrence().unwrap().0,
            Recurrence::Daily {
                at: time(10, 0, 0, 0)
            }
        );
    }
}
