//! Everything the bot says.
//!
//! Isolated in one module so the wording can be changed, or the bot translated, by editing
//! a single file. The copy is English; what the *user* writes may be in any language, and
//! their own words are echoed back untouched — see [`created`] and [`listing`], which
//! interpolate `reminder.text` exactly as it was stored.

use serana_domain::reminder::{MonthDays, Recurrence, Reminder, Weekday};
use serana_services::ReminderOutcome;

pub const HELP: &str = "\
Serana — your reminders, in plain words.

/reminder — set, change, remove or finish one
/reminders — list them
/compact — fold this conversation up into a summary
/help — this message

Just say what you want:
  /reminder every month on the 20th, issue an invoice
  /reminder move the invoice one to 22:30
  /reminder I already sent the invoice
  /reminder remove the invoice reminder";

pub const REMINDER_NEEDS_TEXT: &str = "Say what to remind you about, and when.\n\
     For example: /reminder every month on the 20th, issue an invoice";

pub const NO_REMINDERS: &str =
    "No reminders yet.\nSet one: /reminder every month on the 20th, issue an invoice";

pub const COMPACTED: &str =
    "🧹 Folded the conversation up into a summary. I still know where we got to.";

pub const NOTHING_TO_COMPACT: &str = "Nothing to fold up yet.";

pub const UNKNOWN_COMMAND: &str =
    "I do not know that command.\nJust say what you want — /help lists what I can do.";

pub const NOT_ALLOWED: &str = "This is a personal bot and does not answer you.";

/// Join phrases the way a person would: "a", "a and b", "a, b and c".
fn list(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// How a checklist line is marked. A tick for finished, a plain circle for not yet —
/// deliberately not a cross, which reads as failed rather than outstanding.
const DONE: &str = "✅";
const PENDING: &str = "⚪";

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

/// Every confirmation has the same shape, so the eye learns where to look: what it is
/// about, then the schedule, then when it next lands, then the id.
///
/// The id goes last and unlabelled — it matters only when something has gone wrong, and a
/// line of machine noise at the top pushes the schedule (the part worth checking) down.
fn confirmation(badge: &str, reminder: &Reminder, when: &str, now: jiff::Timestamp) -> String {
    format!(
        "{badge} {}\n{}\n🗓 {}\n⏰ {when} {}\n🆔 {}",
        reminder.text,
        checklist(reminder, now),
        describe(&reminder.recurrence),
        next_fire(reminder),
        reminder.id
    )
}

/// Confirmation after creating a reminder.
pub fn created(reminder: &Reminder, now: jiff::Timestamp) -> String {
    confirmation("✅", reminder, "next", now)
}

/// One line per reminder.
pub fn listing(reminders: &[Reminder]) -> String {
    if reminders.is_empty() {
        return NO_REMINDERS.to_owned();
    }
    let mut out = String::new();
    for (index, reminder) in reminders.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        // A spent reminder is still listed — it is the one the user is most likely hunting
        // for — but it should not look like something that is still going to happen.
        let bell = if reminder.is_active() { "⏰" } else { "🔕" };
        out.push_str(&format!(
            "{} {}\n   🗓 {}\n   {bell} {}\n   🆔 {}\n",
            if reminder.is_active() { "•" } else { "◦" },
            reminder.text,
            describe(&reminder.recurrence),
            next_fire(reminder),
            reminder.id
        ));
    }
    out
}

/// The id line, for anything the user might reply to or name next.
///
/// Omitted only when the reminder has just been removed: an id that resolves to nothing is
/// worse than no id at all.
fn trailer(reminder: &Reminder, removed: bool) -> String {
    if removed {
        String::new()
    } else {
        format!("\n🆔 {}", reminder.id)
    }
}

/// Confirmation after ticking items off a checklist.
///
/// Shows the whole list, ticks and all, because the question in the user's mind is what is
/// left. Anything the model failed to match is named rather than silently dropped — a line
/// that did not get ticked is exactly the thing they need to know about.
pub fn completed(reminder: &Reminder, ticked: &[String], now: jiff::Timestamp) -> String {
    if reminder.all_done(now).unwrap_or(false) {
        let removed = !reminder.recurrence.is_recurring();
        return format!(
            "✅ All done — {}\n{}\n{}{}",
            reminder.text,
            checklist(reminder, now),
            finished(reminder),
            trailer(reminder, removed)
        );
    }
    let mut out = format!("{}\n{}", reminder.text, checklist(reminder, now));
    if ticked.is_empty() {
        out.push_str("\nNothing matched — say which line, as it is written above.");
    }
    // Still outstanding, so they will very likely reply to this message to tick the next
    // line off. Without the id that reply is guesswork again.
    out.push_str(&trailer(reminder, false));
    out
}

/// Confirmation after marking a period done.
///
/// It names the next firing time because that is the question the user actually has:
/// not "did it register" but "when does this come back".
pub fn acknowledged(reminder: &Reminder) -> String {
    let removed = !reminder.recurrence.is_recurring();
    format!(
        "✔️ Done — {}\n\n{}{}",
        reminder.text,
        finished(reminder),
        trailer(reminder, removed)
    )
}

/// What became of a reminder whose work is finished.
///
/// A repeating one is only resting; a one-off is gone, and saying so matters — otherwise
/// the user goes looking for it in `/reminders` and wonders where it went.
fn finished(reminder: &Reminder) -> String {
    if !reminder.recurrence.is_recurring() {
        return "🗑 Removed — it was a one-off.".to_owned();
    }
    match reminder.next_fire_at {
        Some(_) => format!("🔕 quiet until {}", next_fire(reminder)),
        None => "Nothing further scheduled.".to_owned(),
    }
}

/// A reminder as it arrives when it fires.
///
/// The bell is the whole point: this message is unprompted, so it has to be recognisable as
/// a reminder at a glance rather than read as the assistant talking.
///
/// A checklist shows only what is still outstanding. On the fourth day of a six-day window
/// the two things already done are not news, and repeating them buries the one that is.
pub fn fired(reminder: &Reminder, now: jiff::Timestamp) -> String {
    let outstanding = reminder.outstanding(now).unwrap_or_default();
    if outstanding.is_empty() {
        return format!("⏰ {}\n🆔 {}", reminder.text, reminder.id);
    }
    let lines: Vec<String> = outstanding
        .iter()
        .map(|item| format!("{PENDING} {}", item.text))
        .collect();
    // The id is here so a reply can be resolved exactly. Two reminders worded the same way
    // are indistinguishable from their text alone, and replying to one of them is the
    // gesture most likely to be ambiguous.
    format!(
        "⏰ {}\n\n{}\n🆔 {}",
        reminder.text,
        lines.join("\n"),
        reminder.id
    )
}

/// The checklist in full, ticks and all. Used where the user asked to see it.
fn checklist(reminder: &Reminder, now: jiff::Timestamp) -> String {
    if reminder.items.is_empty() {
        return String::new();
    }
    let lines: Vec<String> = reminder
        .items
        .iter()
        .map(|item| {
            let done = reminder.is_done(item, now).unwrap_or(false);
            format!("{} {}", if done { DONE } else { PENDING }, item.text)
        })
        .collect();
    format!("\n{}\n", lines.join("\n"))
}

pub fn deleted(reminder: &Reminder) -> String {
    format!("🗑 Deleted — {}", reminder.text)
}

pub fn updated(reminder: &Reminder, now: jiff::Timestamp) -> String {
    confirmation("✏️ Updated —", reminder, "next", now)
}

/// What to say after a reminder turn.
///
/// [`ReminderOutcome::Said`] is the model's own words, passed through untouched: it is the
/// "which of these did you mean?" case, and rewriting it would throw away the only thing
/// that tells the user how to answer.
pub fn outcome(outcome: &ReminderOutcome) -> String {
    match outcome {
        ReminderOutcome::Created { reminder, at } => created(reminder, *at),
        ReminderOutcome::Updated { reminder, at } => updated(reminder, *at),
        ReminderOutcome::Deleted(reminder) => deleted(reminder),
        ReminderOutcome::Acknowledged(reminder) => acknowledged(reminder),
        ReminderOutcome::Completed {
            reminder,
            ticked,
            at,
        } => completed(reminder, ticked, *at),
        ReminderOutcome::Said(words) => words.clone(),
    }
}

#[cfg(test)]
mod tests {
    use serana_domain::reminder::{
        MonthDays, ReminderId, TimeZoneName, TodoItem, UserId, WeekDays,
    };

    use super::*;

    fn reminder(recurrence: Recurrence, next_fire_at: Option<&str>) -> Reminder {
        Reminder {
            id: ReminderId::new("a3f9k2xy"),
            owner: UserId::new(1),
            // Deliberately not English: the reminder text is the user's own words, and the
            // copy around it must not assume a script or a language.
            text: "оформить invoice".into(),
            items: Vec::new(),
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
        let message = created(&r, jiff::Timestamp::UNIX_EPOCH);
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
        assert!(created(&r, jiff::Timestamp::UNIX_EPOCH).contains("写发票"));
        assert!(listing(std::slice::from_ref(&r)).contains("写发票"));
        assert!(deleted(&r).contains("写发票"));
    }

    #[test]
    fn every_reply_about_a_living_reminder_carries_its_id() {
        // Whatever the user is looking at, replying to it has to be enough to identify the
        // reminder — two worded the same are otherwise indistinguishable.
        let now = jiff::Timestamp::UNIX_EPOCH;
        let mut r = reminder(
            Recurrence::Monthly {
                days: MonthDays::new([1, 2]).unwrap(),
                at: jiff::civil::time(9, 0, 0, 0),
            },
            Some("2026-03-11T05:00:00Z"),
        );
        r.items = vec![TodoItem::new("one"), TodoItem::new("two")];

        for (what, message) in [
            ("created", created(&r, now)),
            ("updated", updated(&r, now)),
            ("fired", fired(&r, now)),
            ("completed", completed(&r, &["one".into()], now)),
            ("acknowledged", acknowledged(&r)),
            ("listing", listing(std::slice::from_ref(&r))),
        ] {
            assert!(
                message.contains("🆔 a3f9k2xy"),
                "{what} has no id:\n{message}"
            );
        }
    }

    #[test]
    fn a_removed_reminder_is_not_given_an_id_that_resolves_to_nothing() {
        let now = jiff::Timestamp::UNIX_EPOCH;
        let mut r = reminder(
            Recurrence::Once {
                at: jiff::civil::date(2026, 3, 15).at(9, 0, 0, 0),
            },
            None,
        );
        r.items = vec![TodoItem {
            text: "one".into(),
            done_at: Some(now),
        }];

        let message = completed(&r, &["one".into()], now);
        assert!(message.contains("Removed"), "{message}");
        assert!(!message.contains("🆔"), "{message}");
        assert!(!acknowledged(&r).contains("🆔"), "{}", acknowledged(&r));
    }

    #[test]
    fn a_delivery_carries_its_id_so_a_reply_can_be_resolved() {
        // Two reminders worded the same are indistinguishable from their text, and a reply
        // quotes text. The id is the only thing that tells them apart.
        let mut r = reminder(
            Recurrence::Daily {
                at: jiff::civil::time(9, 0, 0, 0),
            },
            Some("2026-03-11T05:00:00Z"),
        );
        r.text = "mortgage".into();
        r.items = vec![TodoItem::new("exchange money")];

        let message = fired(&r, jiff::Timestamp::UNIX_EPOCH);
        assert!(message.contains("a3f9k2xy"), "{message}");
        assert!(message.contains("⚪ exchange money"), "{message}");

        // And on a plain reminder too, where there is no checklist to hang it off.
        r.items.clear();
        assert!(fired(&r, jiff::Timestamp::UNIX_EPOCH).contains("a3f9k2xy"));
    }

    #[test]
    fn a_confirmation_shows_the_checklist_it_just_set_up() {
        // The bug this replaced: "Updated" with no sign of the list that was added, so
        // there was no way to tell whether it had worked.
        let mut r = reminder(
            Recurrence::Monthly {
                days: MonthDays::new([1, 2, 3]).unwrap(),
                at: jiff::civil::time(12, 0, 0, 0),
            },
            Some("2026-04-01T08:00:00Z"),
        );
        r.text = "monthly payment".into();
        r.items = vec![
            TodoItem::new("exchange money"),
            TodoItem::new("transfer to tbc"),
        ];

        let message = created(&r, jiff::Timestamp::UNIX_EPOCH);
        assert!(message.contains("⚪ exchange money"), "{message}");
        assert!(message.contains("⚪ transfer to tbc"), "{message}");
        assert!(message.contains("monthly payment"), "{message}");
    }

    #[test]
    fn a_confirmation_keeps_the_ticks_an_edit_preserved() {
        let now = "2026-03-02T08:00:00Z".parse::<jiff::Timestamp>().unwrap();
        let mut r = reminder(
            Recurrence::Monthly {
                days: MonthDays::new([1, 2, 3]).unwrap(),
                at: jiff::civil::time(12, 0, 0, 0),
            },
            Some("2026-03-03T08:00:00Z"),
        );
        r.items = vec![
            TodoItem {
                text: "exchange money".into(),
                done_at: Some(now),
            },
            TodoItem::new("transfer to tbc"),
        ];

        let message = updated(&r, now);
        assert!(message.contains("✅ exchange money"), "{message}");
        assert!(message.contains("⚪ transfer to tbc"), "{message}");
    }

    #[test]
    fn a_plain_reminder_gains_no_empty_checklist_block() {
        let r = reminder(
            Recurrence::Daily {
                at: jiff::civil::time(9, 0, 0, 0),
            },
            Some("2026-03-11T05:00:00Z"),
        );
        let message = created(&r, jiff::Timestamp::UNIX_EPOCH);
        assert!(!message.contains('⚪'), "{message}");
        assert!(
            !message.contains("\n\n\n"),
            "no gap where the list would be: {message}"
        );
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
        assert_eq!(message.matches("🆔 a3f9k2xy").count(), 2, "{message}");
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
        assert!(created(&r, jiff::Timestamp::UNIX_EPOCH).contains("never"));
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
            created(&r, jiff::Timestamp::UNIX_EPOCH).contains("2026-03-11"),
            "falls back to the raw instant"
        );
    }
}
