//! What the model is asked to produce for a reminder, on top of the shared schedule.
//!
//! The scheduling half lives in [`crate::schedule`] — a reminder and a price digest repeat
//! on the same vocabulary. What is left here is what only a reminder has: the words, and
//! the checklist.

use schemars::JsonSchema;
use serde::Deserialize;

use serana_domain::reminder::{Recurrence, TodoItem};

use crate::schedule::ParsedSchedule;

use super::ReminderError;

/// The shape the model is asked to produce.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct ParsedReminder {
    /// Flattened, so the model fills one flat object rather than a nested one — weaker
    /// models fill nested arguments badly, and the schedule fields are the ones they get
    /// wrong first.
    #[serde(flatten)]
    pub schedule: ParsedSchedule,
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

impl ParsedReminder {
    /// Validate and convert. The returned `String` is the reminder's own text.
    pub(crate) fn into_recurrence(
        self,
    ) -> Result<(Recurrence, String, Vec<TodoItem>), ReminderError> {
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

        let recurrence = self
            .schedule
            .into_recurrence()
            .map_err(ReminderError::Unparsable)?;
        Ok((recurrence, text, items))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::{ParsedKind, ParsedSchedule, ParsedWeekday, parse_time};
    use jiff::civil::time;
    use serana_domain::reminder::{MonthDays, WeekDays, Weekday};

    fn parsed(kind: ParsedKind) -> ParsedReminder {
        ParsedReminder {
            schedule: ParsedSchedule {
                kind,
                time: "10:00".into(),
                weekdays: None,
                days_of_month: None,
                date: None,
            },
            items: None,
            text: "оформить invoice".into(),
        }
    }

    #[test]
    fn the_brief_example_becomes_a_monthly_recurrence() {
        let mut p = parsed(ParsedKind::Monthly);
        p.schedule.days_of_month = Some(vec![20]);
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
        p.schedule.weekdays = Some(vec![ParsedWeekday::Friday]);
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
        p.schedule.date = Some("2026-03-20".into());
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
            p.schedule.days_of_month = Some(vec![day]);
            assert!(p.into_recurrence().is_err(), "{day} should be refused");
        }
    }

    #[test]
    fn valid_days_of_the_month_are_accepted_at_the_boundaries() {
        for day in [1, 28, 31] {
            let mut p = parsed(ParsedKind::Monthly);
            p.schedule.days_of_month = Some(vec![day]);
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
        p.schedule.date = Some("20 марта".into());
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
        p.schedule.weekdays = Some(vec![ParsedWeekday::Friday]);
        p.schedule.days_of_month = Some(vec![20]);
        p.schedule.date = Some("2026-03-20".into());
        assert_eq!(
            p.into_recurrence().unwrap().0,
            Recurrence::Daily {
                at: time(10, 0, 0, 0)
            }
        );
    }
}
