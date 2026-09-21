//! The system prompt for a reminder turn.
//!
//! It carries two things the model cannot know on its own: what time it is locally, and
//! which reminders the person already has. The second is what makes "remove the salary
//! invoice one" resolvable — the model matches the phrase against real rows and answers
//! with an id it was given, rather than inventing one.

use serana_domain::reminder::{Reminder, TimeZoneName};

/// Existing reminders, as compact JSON.
///
/// JSON rather than prose because the model reads it more reliably, and because rendering
/// a schedule in English belongs to the frontend — this crate must not grow a second copy
/// of it that can drift from the one users read.
fn existing(reminders: &[Reminder]) -> String {
    if reminders.is_empty() {
        return "The person has no reminders yet.".to_owned();
    }
    let rows: Vec<serde_json::Value> = reminders
        .iter()
        .map(|reminder| {
            serde_json::json!({
                "id": reminder.id.as_str(),
                "text": reminder.text,
                "schedule": reminder.recurrence,
                "active": reminder.is_active(),
            })
        })
        .collect();
    format!(
        "The person's existing reminders, as JSON:\n{}",
        serde_json::Value::Array(rows)
    )
}

/// Build the prompt for a turn issued at `local` in `timezone`.
pub(crate) fn build(
    local: &jiff::Zoned,
    timezone: &TimeZoneName,
    reminders: &[Reminder],
) -> String {
    format!(
        "You manage a person's reminders. Read what they ask for and call exactly one \
         function.\n\
         \n\
         Their current local date and time is {date} {time} ({weekday}), time zone {zone}.\n\
         Resolve relative expressions such as \"tomorrow\", \"next Friday\" or \"in an hour\" \
         against that moment.\n\
         \n\
         {existing}\n\
         \n\
         Choosing what to do:\n\
         - they describe something new -> create_reminder\n\
         - they want an existing one changed -> update_reminder, with its id and the \
         complete new schedule\n\
         - they want one gone for good -> delete_reminder\n\
         - they say they have already done the thing -> acknowledge_reminder, which silences \
         it until the next period instead of deleting it\n\
         \n\
         Only ever use an id from the list above. If nothing in the list matches what they \
         referred to, or if several match and you cannot tell which they mean, do NOT guess: \
         answer in one short sentence naming the candidates and asking which they meant.\n\
         \n\
         Keep the reminder text in the language the person wrote it in, and keep it in their \
         own words. Strip the scheduling part out of it: \"every month on the 20th, tell me \
         to issue an invoice\" has the text \"issue an invoice\".\n\
         If no time of day is given, use 09:00.\n\
         \n\
         A weekly or monthly reminder takes a LIST of days. Say exactly which days it fires \
         on:\n\
         - a single day is a list of one: \"on the 20th\" is [20]\n\
         - write a range out in full: \"from the 20th to the 26th\" is \
         [20, 21, 22, 23, 24, 25, 26]\n\
         - days need not be adjacent: \"on the 20th, and 5 days before\" is [15, 20], and \
         \"on the 1st and the 15th\" is [1, 15]",
        date = local.date(),
        time = local.time().strftime("%H:%M"),
        weekday = local.strftime("%A"),
        zone = timezone,
        existing = existing(reminders),
    )
}

#[cfg(test)]
mod tests {
    use serana_domain::reminder::{MonthDays, Recurrence, Reminder, ReminderId, UserId};

    use super::*;

    fn at(raw: &str) -> jiff::Zoned {
        raw.parse::<jiff::Timestamp>()
            .unwrap()
            .to_zoned(jiff::tz::TimeZone::get("Asia/Tbilisi").unwrap())
    }

    fn reminder(id: &str, text: &str) -> Reminder {
        Reminder {
            id: ReminderId::new(id),
            owner: UserId::new(1),
            text: text.into(),
            recurrence: Recurrence::Monthly {
                days: MonthDays::new([20]).unwrap(),
                at: jiff::civil::time(9, 0, 0, 0),
            },
            timezone: TimeZoneName::new("Asia/Tbilisi"),
            created_at: jiff::Timestamp::UNIX_EPOCH,
            next_fire_at: None,
            last_fired_at: None,
            acknowledged_through: None,
        }
    }

    #[test]
    fn the_prompt_carries_the_local_moment() {
        // Without it, "tomorrow" cannot be resolved at all.
        let prompt = build(
            &at("2026-03-10T06:00:00Z"),
            &TimeZoneName::new("Asia/Tbilisi"),
            &[],
        );
        assert!(prompt.contains("2026-03-10"), "{prompt}");
        assert!(prompt.contains("10:00"), "{prompt}");
        assert!(prompt.contains("Tuesday"), "{prompt}");
        assert!(prompt.contains("Asia/Tbilisi"), "{prompt}");
    }

    #[test]
    fn existing_reminders_reach_the_model_with_their_ids() {
        // This is what makes "remove the salary invoice one" resolvable.
        let prompt = build(
            &at("2026-03-10T06:00:00Z"),
            &TimeZoneName::new("Asia/Tbilisi"),
            &[
                reminder("31k8v7wt", "send salary invoice"),
                reminder("7g2pq0aa", "pay rent"),
            ],
        );
        assert!(prompt.contains("31k8v7wt"), "{prompt}");
        assert!(prompt.contains("send salary invoice"), "{prompt}");
        assert!(prompt.contains("7g2pq0aa"), "{prompt}");
        assert!(prompt.contains("\"kind\":\"monthly\""), "{prompt}");
    }

    #[test]
    fn an_empty_list_says_so_rather_than_showing_an_empty_array() {
        // `[]` invites the model to treat an id as optional; a sentence does not.
        let prompt = build(
            &at("2026-03-10T06:00:00Z"),
            &TimeZoneName::new("Asia/Tbilisi"),
            &[],
        );
        assert!(prompt.contains("no reminders yet"), "{prompt}");
    }

    #[test]
    fn the_model_is_told_to_ask_rather_than_guess_between_candidates() {
        let prompt = build(
            &at("2026-03-10T06:00:00Z"),
            &TimeZoneName::new("Asia/Tbilisi"),
            &[],
        );
        assert!(prompt.contains("do NOT guess"), "{prompt}");
    }
}
