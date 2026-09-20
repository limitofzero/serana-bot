//! Everything the bot says.
//!
//! Isolated in one module so the bot can be translated by editing a single file. The copy
//! is Russian because that is the language its user writes in; identifiers and comments
//! stay English like the rest of the codebase.

use serana_domain::reminder::{Recurrence, Reminder, Weekday};

pub const HELP: &str = "\
Я Serana — ваш ассистент.

/reminder <описание> — поставить напоминание обычной фразой
/reminders — показать все напоминания
/reminder_delete <id> — удалить напоминание
/help — эта справка

Например:
/reminder каждый месяц 20 число - писать мне что надо оформить invoice";

pub const REMINDER_NEEDS_TEXT: &str = "После /reminder напишите, о чём и когда напоминать.\n\
     Например: /reminder каждый месяц 20 число - оформить invoice";

pub const DELETE_NEEDS_ID: &str =
    "После /reminder_delete укажите id напоминания.\nПосмотреть их можно через /reminders";

pub const NO_REMINDERS: &str = "Напоминаний пока нет. Поставить — /reminder";

pub const NOT_ALLOWED: &str = "Этот бот личный и вам не отвечает.";

/// Weekday phrases stored whole, so gender agreement cannot be got wrong by assembling
/// "каждый" with the wrong noun.
fn every_weekday(weekday: Weekday) -> &'static str {
    match weekday {
        Weekday::Monday => "каждый понедельник",
        Weekday::Tuesday => "каждый вторник",
        Weekday::Wednesday => "каждую среду",
        Weekday::Thursday => "каждый четверг",
        Weekday::Friday => "каждую пятницу",
        Weekday::Saturday => "каждую субботу",
        Weekday::Sunday => "каждое воскресенье",
    }
}

const MONTHS: [&str; 12] = [
    "января",
    "февраля",
    "марта",
    "апреля",
    "мая",
    "июня",
    "июля",
    "августа",
    "сентября",
    "октября",
    "ноября",
    "декабря",
];

fn hhmm(time: jiff::civil::Time) -> String {
    format!("{:02}:{:02}", time.hour(), time.minute())
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
                "один раз, {} {} в {}",
                day_and_month(at.date()),
                at.year(),
                hhmm(at.time())
            )
        }
        Recurrence::Daily { at } => format!("каждый день в {}", hhmm(*at)),
        Recurrence::Weekly { weekday, at } => {
            format!("{} в {}", every_weekday(*weekday), hhmm(*at))
        }
        Recurrence::Monthly { day, at } => {
            format!("каждое {day} число в {}", hhmm(*at))
        }
    }
}

/// When a reminder next fires, in its own zone.
fn next_fire(reminder: &Reminder) -> String {
    let Some(at) = reminder.next_fire_at else {
        return "не сработает".to_owned();
    };
    let Ok(tz) = reminder.timezone.resolve() else {
        return at.to_string();
    };
    let local = at.to_zoned(tz);
    format!(
        "{} {} в {}",
        day_and_month(local.date()),
        local.year(),
        hhmm(local.time())
    )
}

/// Confirmation after creating a reminder.
pub fn created(reminder: &Reminder) -> String {
    format!(
        "Поставил: {}\nРасписание: {}\nБлижайшее: {}\nid: {}",
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
    let mut out = String::from("Ваши напоминания:\n");
    for reminder in reminders {
        out.push_str(&format!(
            "\n• {}\n  {} — ближайшее {}\n  id: {}\n",
            reminder.text,
            describe(&reminder.recurrence),
            next_fire(reminder),
            reminder.id
        ));
    }
    out
}

pub fn deleted(reminder: &Reminder) -> String {
    format!("Удалил: {}", reminder.text)
}

#[cfg(test)]
mod tests {
    use serana_domain::reminder::{ReminderId, TimeZoneName, UserId};

    use super::*;

    fn reminder(recurrence: Recurrence, next_fire_at: Option<&str>) -> Reminder {
        Reminder {
            id: ReminderId::new("a3f9k2xy"),
            owner: UserId::new(1),
            text: "оформить invoice".into(),
            recurrence,
            timezone: TimeZoneName::new("Asia/Tbilisi"),
            created_at: jiff::Timestamp::UNIX_EPOCH,
            next_fire_at: next_fire_at.map(|t| t.parse().unwrap()),
            last_fired_at: None,
        }
    }

    #[test]
    fn a_monthly_schedule_reads_back_as_the_user_asked_for_it() {
        let recurrence = Recurrence::Monthly {
            day: 20,
            at: jiff::civil::time(10, 0, 0, 0),
        };
        assert_eq!(describe(&recurrence), "каждое 20 число в 10:00");
    }

    #[test]
    fn a_daily_schedule_pads_the_time() {
        let recurrence = Recurrence::Daily {
            at: jiff::civil::time(9, 5, 0, 0),
        };
        assert_eq!(describe(&recurrence), "каждый день в 09:05");
    }

    #[test]
    fn every_weekday_agrees_in_gender() {
        // "каждый среду" would be wrong; the phrases are stored whole to prevent it.
        let cases = [
            (Weekday::Monday, "каждый понедельник"),
            (Weekday::Wednesday, "каждую среду"),
            (Weekday::Friday, "каждую пятницу"),
            (Weekday::Sunday, "каждое воскресенье"),
        ];
        for (weekday, expected) in cases {
            let recurrence = Recurrence::Weekly {
                weekday,
                at: jiff::civil::time(18, 0, 0, 0),
            };
            assert_eq!(describe(&recurrence), format!("{expected} в 18:00"));
        }
    }

    #[test]
    fn a_one_off_names_its_month() {
        let recurrence = Recurrence::Once {
            at: jiff::civil::date(2026, 3, 20).at(10, 0, 0, 0),
        };
        assert_eq!(describe(&recurrence), "один раз, 20 марта 2026 в 10:00");
    }

    #[test]
    fn every_month_has_a_name() {
        for month in 1..=12 {
            let date = jiff::civil::date(2026, month, 1);
            assert!(!day_and_month(date).is_empty(), "month {month}");
        }
        assert_eq!(day_and_month(jiff::civil::date(2026, 12, 31)), "31 декабря");
        assert_eq!(day_and_month(jiff::civil::date(2026, 1, 1)), "1 января");
    }

    #[test]
    fn a_confirmation_shows_the_schedule_the_next_firing_and_the_id() {
        let r = reminder(
            Recurrence::Monthly {
                day: 20,
                at: jiff::civil::time(10, 0, 0, 0),
            },
            Some("2026-03-20T06:00:00Z"),
        );
        let message = created(&r);
        assert!(message.contains("оформить invoice"), "{message}");
        assert!(message.contains("каждое 20 число в 10:00"), "{message}");
        // 06:00 UTC is 10:00 in Tbilisi; the user is shown their own time.
        assert!(message.contains("20 марта 2026 в 10:00"), "{message}");
        assert!(message.contains("a3f9k2xy"), "{message}");
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
                    day: 20,
                    at: jiff::civil::time(10, 0, 0, 0),
                },
                Some("2026-03-20T06:00:00Z"),
            ),
        ];
        let message = listing(&reminders);
        assert_eq!(message.matches("id: a3f9k2xy").count(), 2, "{message}");
        assert!(message.contains("каждый день в 09:00"), "{message}");
        assert!(message.contains("каждое 20 число в 10:00"), "{message}");
    }

    #[test]
    fn a_spent_reminder_says_it_will_not_fire_rather_than_showing_a_blank() {
        let r = reminder(
            Recurrence::Daily {
                at: jiff::civil::time(9, 0, 0, 0),
            },
            None,
        );
        assert!(created(&r).contains("не сработает"));
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
