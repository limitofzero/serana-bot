//! Everything the bot says.
//!
//! Isolated in one module so the wording can be changed, or the bot translated, by editing
//! a single file. The copy is English; what the *user* writes may be in any language, and
//! their own words are echoed back untouched — see [`created`] and [`listing`], which
//! interpolate `reminder.text` exactly as it was stored.

use serana_domain::reminder::{MonthDays, Recurrence, Reminder, Weekday};
use serana_services::ReminderOutcome;

pub const HELP: &str = "\
I am Serana, your assistant.

/reminder <anything> — set, change, remove or finish a reminder, in plain words
/reminders — list your reminders
/help — this message

For example:
/reminder every month on the 20th, tell me to issue an invoice";

pub const REMINDER_NEEDS_TEXT: &str = "Say what to remind you about, and when.\n\
     For example: /reminder every month on the 20th, issue an invoice";

pub const DELETE_NEEDS_ID: &str =
    "Give the id of the reminder to delete.\nYou can see them with /reminders";

pub const DONE_NEEDS_ID: &str =
    "Give the id of the reminder you have dealt with.\nYou can see them with /reminders";

pub const NO_REMINDERS: &str = "No reminders yet. Set one with /reminder";

pub const NOT_ALLOWED: &str = "This is a personal bot and does not answer you.";

/// Join phrases the way a person would: "a", "a and b", "a, b and c".
fn list(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn weekday_name(weekday: Weekday) -> &'static str {
    match weekday {
        Weekday::Monday => "Monday",
        Weekday::Tuesday => "Tuesday",
        Weekday::Wednesday => "Wednesday",
        Weekday::Thursday => "Thursday",
        Weekday::Friday => "Friday",
        Weekday::Saturday => "Saturday",
        Weekday::Sunday => "Sunday",
    }
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

fn hhmm(time: jiff::civil::Time) -> String {
    format!("{:02}:{:02}", time.hour(), time.minute())
}

/// `20` becomes `20th`, `21` becomes `21st`.
///
/// The teens are the exception the naive rule gets wrong: 11, 12 and 13 take "th" despite
/// ending in 1, 2 and 3.
fn ordinal(day: i8) -> String {
    let suffix = match (day % 10, day % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{day}{suffix}")
}

fn day_and_month(date: jiff::civil::Date) -> String {
    // `month()` is 1-12, so the index is always in range.
    format!(
        "{} {}",
        date.day(),
        MONTHS[usize::from(date.month() as u8) - 1]
    )
}

/// A recurrence in words — the reason schedules are stored as an enum rather than as cron.
pub fn describe(recurrence: &Recurrence) -> String {
    match recurrence {
        Recurrence::Once { at } => {
            format!(
                "once, on {} {} at {}",
                day_and_month(at.date()),
                at.year(),
                hhmm(at.time())
            )
        }
        Recurrence::Daily { at } => format!("every day at {}", hhmm(*at)),
        Recurrence::Weekly { days, at } => {
            let named: Vec<String> = days.iter().map(|d| weekday_name(d).to_owned()).collect();
            format!("every {} at {}", list(&named), hhmm(*at))
        }
        Recurrence::Monthly { days, at } => {
            format!("every month on the {} at {}", month_days(days), hhmm(*at))
        }
    }
}

/// Days of the month, as a range when they form one.
///
/// "the 20th to the 26th" rather than "the 20th, 21st, 22nd, 23rd, 24th, 25th and 26th" —
/// the readback exists to be checked at a glance, and a seven-item list is not.
fn month_days(days: &MonthDays) -> String {
    let all: Vec<i8> = days.iter().collect();
    let contiguous = all.len() > 2 && all.windows(2).all(|pair| pair[1] == pair[0] + 1);
    if contiguous {
        return format!("{} to the {}", ordinal(all[0]), ordinal(all[all.len() - 1]));
    }
    let named: Vec<String> = all.into_iter().map(ordinal).collect();
    list(&named)
}

/// When a reminder next fires, in its own zone.
fn next_fire(reminder: &Reminder) -> String {
    let Some(at) = reminder.next_fire_at else {
        return "never".to_owned();
    };
    let Ok(tz) = reminder.timezone.resolve() else {
        return at.to_string();
    };
    let local = at.to_zoned(tz);
    format!(
        "{} {} at {}",
        day_and_month(local.date()),
        local.year(),
        hhmm(local.time())
    )
}

/// Confirmation after creating a reminder.
pub fn created(reminder: &Reminder) -> String {
    format!(
        "Set: {}\nSchedule: {}\nNext: {}\nid: {}",
        reminder.text,
        describe(&reminder.recurrence),
        next_fire(reminder),
        reminder.id
    )
}

/// One line per reminder.
pub fn listing(reminders: &[Reminder]) -> String {
    if reminders.is_empty() {
        return NO_REMINDERS.to_owned();
    }
    let mut out = String::from("Your reminders:\n");
    for reminder in reminders {
        out.push_str(&format!(
            "\n• {}\n  {} — next {}\n  id: {}\n",
            reminder.text,
            describe(&reminder.recurrence),
            next_fire(reminder),
            reminder.id
        ));
    }
    out
}

/// Confirmation after marking a period done.
///
/// It names the next firing time because that is the question the user actually has:
/// not "did it register" but "when does this come back".
pub fn acknowledged(reminder: &Reminder) -> String {
    match reminder.next_fire_at {
        Some(_) => format!("Done: {}\nBack on: {}", reminder.text, next_fire(reminder)),
        None => format!("Done: {}\nNothing further scheduled.", reminder.text),
    }
}

pub fn deleted(reminder: &Reminder) -> String {
    format!("Deleted: {}", reminder.text)
}

pub fn updated(reminder: &Reminder) -> String {
    format!(
        "Updated: {}\nSchedule: {}\nNext: {}\nid: {}",
        reminder.text,
        describe(&reminder.recurrence),
        next_fire(reminder),
        reminder.id
    )
}

/// What to say after a reminder turn.
///
/// [`ReminderOutcome::Said`] is the model's own words, passed through untouched: it is the
/// "which of these did you mean?" case, and rewriting it would throw away the only thing
/// that tells the user how to answer.
pub fn outcome(outcome: &ReminderOutcome) -> String {
    match outcome {
        ReminderOutcome::Created(reminder) => created(reminder),
        ReminderOutcome::Updated(reminder) => updated(reminder),
        ReminderOutcome::Deleted(reminder) => deleted(reminder),
        ReminderOutcome::Acknowledged(reminder) => acknowledged(reminder),
        ReminderOutcome::Said(words) => words.clone(),
    }
}

#[cfg(test)]
mod tests {
    use serana_domain::reminder::{MonthDays, ReminderId, TimeZoneName, UserId, WeekDays};

    use super::*;

    fn reminder(recurrence: Recurrence, next_fire_at: Option<&str>) -> Reminder {
        Reminder {
            id: ReminderId::new("a3f9k2xy"),
            owner: UserId::new(1),
            // Deliberately not English: the reminder text is the user's own words, and the
            // copy around it must not assume a script or a language.
            text: "оформить invoice".into(),
            recurrence,
            timezone: TimeZoneName::new("Asia/Tbilisi"),
            created_at: jiff::Timestamp::UNIX_EPOCH,
            next_fire_at: next_fire_at.map(|t| t.parse().unwrap()),
            last_fired_at: None,
            acknowledged_through: None,
        }
    }

    #[test]
    fn a_monthly_schedule_reads_back_as_the_user_asked_for_it() {
        let recurrence = Recurrence::Monthly {
            days: MonthDays::new([20]).unwrap(),
            at: jiff::civil::time(10, 0, 0, 0),
        };
        assert_eq!(describe(&recurrence), "every month on the 20th at 10:00");
    }

    #[test]
    fn a_daily_schedule_pads_the_time() {
        let recurrence = Recurrence::Daily {
            at: jiff::civil::time(9, 5, 0, 0),
        };
        assert_eq!(describe(&recurrence), "every day at 09:05");
    }

    #[test]
    fn every_weekday_is_named() {
        let cases = [
            (Weekday::Monday, "Monday"),
            (Weekday::Wednesday, "Wednesday"),
            (Weekday::Friday, "Friday"),
            (Weekday::Sunday, "Sunday"),
        ];
        for (weekday, expected) in cases {
            let recurrence = Recurrence::Weekly {
                days: WeekDays::new([weekday]).unwrap(),
                at: jiff::civil::time(18, 0, 0, 0),
            };
            assert_eq!(describe(&recurrence), format!("every {expected} at 18:00"));
        }
    }

    #[test]
    fn several_weekdays_read_as_a_list_in_calendar_order() {
        let recurrence = Recurrence::Weekly {
            // Given out of order on purpose: the set sorts them.
            days: WeekDays::new([Weekday::Friday, Weekday::Monday, Weekday::Wednesday]).unwrap(),
            at: jiff::civil::time(18, 0, 0, 0),
        };
        assert_eq!(
            describe(&recurrence),
            "every Monday, Wednesday and Friday at 18:00"
        );
    }

    #[test]
    fn a_contiguous_run_of_days_reads_as_a_range() {
        // The whole point of the readback is that a wrong schedule is obvious. Seven
        // ordinals in a row are not.
        let recurrence = Recurrence::Monthly {
            days: MonthDays::new([20, 21, 22, 23, 24, 25, 26]).unwrap(),
            at: jiff::civil::time(22, 30, 0, 0),
        };
        assert_eq!(
            describe(&recurrence),
            "every month on the 20th to the 26th at 22:30"
        );
    }

    #[test]
    fn scattered_days_are_listed_rather_than_ranged() {
        let recurrence = Recurrence::Monthly {
            days: MonthDays::new([1, 15]).unwrap(),
            at: jiff::civil::time(9, 0, 0, 0),
        };
        assert_eq!(
            describe(&recurrence),
            "every month on the 1st and 15th at 09:00"
        );

        let three = Recurrence::Monthly {
            days: MonthDays::new([1, 10, 20]).unwrap(),
            at: jiff::civil::time(9, 0, 0, 0),
        };
        assert_eq!(
            describe(&three),
            "every month on the 1st, 10th and 20th at 09:00"
        );
    }

    #[test]
    fn ordinals_follow_english_rules_including_the_teens() {
        let cases = [
            (1, "1st"),
            (2, "2nd"),
            (3, "3rd"),
            (4, "4th"),
            (11, "11th"),
            (12, "12th"),
            (13, "13th"),
            (20, "20th"),
            (21, "21st"),
            (22, "22nd"),
            (23, "23rd"),
            (30, "30th"),
            (31, "31st"),
        ];
        for (day, expected) in cases {
            assert_eq!(ordinal(day), expected, "day {day}");
        }
    }

    #[test]
    fn every_day_of_the_month_gets_an_ordinal() {
        for day in 1..=31 {
            let rendered = ordinal(day);
            assert!(rendered.starts_with(&day.to_string()), "day {day}");
            assert_eq!(rendered.len(), day.to_string().len() + 2, "day {day}");
        }
    }

    #[test]
    fn a_one_off_names_its_month() {
        let recurrence = Recurrence::Once {
            at: jiff::civil::date(2026, 3, 20).at(10, 0, 0, 0),
        };
        assert_eq!(describe(&recurrence), "once, on 20 March 2026 at 10:00");
    }

    #[test]
    fn every_month_has_a_name() {
        for month in 1..=12 {
            let date = jiff::civil::date(2026, month, 1);
            assert!(!day_and_month(date).is_empty(), "month {month}");
        }
        assert_eq!(
            day_and_month(jiff::civil::date(2026, 12, 31)),
            "31 December"
        );
        assert_eq!(day_and_month(jiff::civil::date(2026, 1, 1)), "1 January");
    }

    #[test]
    fn a_confirmation_shows_the_schedule_the_next_firing_and_the_id() {
        let r = reminder(
            Recurrence::Monthly {
                days: MonthDays::new([20]).unwrap(),
                at: jiff::civil::time(10, 0, 0, 0),
            },
            Some("2026-03-20T06:00:00Z"),
        );
        let message = created(&r);
        assert!(message.contains("оформить invoice"), "{message}");
        assert!(
            message.contains("every month on the 20th at 10:00"),
            "{message}"
        );
        // 06:00 UTC is 10:00 in Tbilisi; the user is shown their own time.
        assert!(message.contains("20 March 2026 at 10:00"), "{message}");
        assert!(message.contains("a3f9k2xy"), "{message}");
    }

    #[test]
    fn the_users_own_words_are_echoed_back_in_their_own_language() {
        // The copy is English; the reminder text is not translated, transliterated or
        // otherwise touched on its way back to the user.
        let mut r = reminder(
            Recurrence::Daily {
                at: jiff::civil::time(9, 0, 0, 0),
            },
            Some("2026-03-11T05:00:00Z"),
        );
        r.text = "写发票".into();
        assert!(created(&r).contains("写发票"));
        assert!(listing(std::slice::from_ref(&r)).contains("写发票"));
        assert!(deleted(&r).contains("写发票"));
    }

    #[test]
    fn an_empty_listing_says_so_instead_of_showing_a_bare_header() {
        assert_eq!(listing(&[]), NO_REMINDERS);
    }

    #[test]
    fn a_listing_has_one_entry_per_reminder_with_its_id() {
        let reminders = vec![
            reminder(
                Recurrence::Daily {
                    at: jiff::civil::time(9, 0, 0, 0),
                },
                Some("2026-03-11T05:00:00Z"),
            ),
            reminder(
                Recurrence::Monthly {
                    days: MonthDays::new([20]).unwrap(),
                    at: jiff::civil::time(10, 0, 0, 0),
                },
                Some("2026-03-20T06:00:00Z"),
            ),
        ];
        let message = listing(&reminders);
        assert_eq!(message.matches("id: a3f9k2xy").count(), 2, "{message}");
        assert!(message.contains("every day at 09:00"), "{message}");
        assert!(
            message.contains("every month on the 20th at 10:00"),
            "{message}"
        );
    }

    #[test]
    fn a_spent_reminder_says_it_will_not_fire_rather_than_showing_a_blank() {
        let r = reminder(
            Recurrence::Daily {
                at: jiff::civil::time(9, 0, 0, 0),
            },
            None,
        );
        assert!(created(&r).contains("never"));
    }

    #[test]
    fn a_broken_zone_still_renders_something_readable() {
        let mut r = reminder(
            Recurrence::Daily {
                at: jiff::civil::time(9, 0, 0, 0),
            },
            Some("2026-03-11T05:00:00Z"),
        );
        r.timezone = TimeZoneName::new("Mars/Olympus_Mons");
        assert!(
            created(&r).contains("2026-03-11"),
            "falls back to the raw instant"
        );
    }
}
