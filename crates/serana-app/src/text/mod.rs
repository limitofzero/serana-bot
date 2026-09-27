//! Everything the bot says.
//!
//! Isolated here so the wording can be changed, or the bot translated, without touching
//! anything that decides *what* to say. What is in this file is what belongs to no one
//! subject; the rest sits with the subject it talks about.

pub mod calendar;
pub mod format;
pub mod prices;
pub mod reminders;

use crate::command::Topic;

/// Which subject one of our own messages was about, when it can be told.
///
/// People reply to a message to point at it, and the message they point at is very often a
/// reminder the scheduler pushed while they were talking about something else entirely.
/// Anything naming a reminder or an event carries its subject's id marker, so the reply
/// goes back to the conversation that message came from rather than to whatever came last.
pub fn topic_of(message: &str) -> Option<Topic> {
    if message.contains(reminders::ID) {
        Some(Topic::Reminders)
    } else if message.contains(calendar::ID) {
        Some(Topic::Calendar)
    } else {
        None
    }
}

pub const HELP: &str = "\
Serana — your reminders and your calendar, in plain words.

/reminder — set, change, remove or finish one
/reminders — list them
/calendar — what is on, book something, call something off
/prices — crypto prices, and when the daily digest lands
/compact — fold this conversation up into a summary
/help — this message

Just say what you want:
  /reminder every month on the 20th, issue an invoice
  /reminder move the invoice one to 22:30
  /reminder I already sent the invoice
  /calendar what have I got on Friday?
  /calendar dentist on Friday at 3, Chavchavadze 1
  /calendar I am not going to Monday\'s standup
  /prices what is COW doing?
  /prices send the digest at 8 instead";

pub const COMPACTED: &str =
    "🧹 Folded the conversation up into a summary. I still know where we got to.";

pub const NOTHING_TO_COMPACT: &str = "Nothing to fold up yet.";

pub const UNKNOWN_COMMAND: &str =
    "I do not know that command.\nJust say what you want — /help lists what I can do.";

pub const NOT_ALLOWED: &str = "This is a personal bot and does not answer you.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_delivered_reminder_is_recognised_as_one() {
        // The flow this exists for: a reminder arrives unprompted while the person was last
        // talking about their calendar, and they reply to it.
        let delivered = reminders::fired(&fixture(), jiff::Timestamp::UNIX_EPOCH);
        assert_eq!(topic_of(&delivered), Some(Topic::Reminders));
    }

    #[test]
    fn a_booking_confirmation_is_recognised_as_an_event() {
        let booked = calendar::created(
            &serana_domain::calendar::CalendarEvent {
                id: serana_domain::calendar::EventId::new("ev1"),
                summary: "dentist".into(),
                starts_at: "2026-09-26T10:00:00Z".parse().unwrap(),
                ends_at: "2026-09-26T11:00:00Z".parse().unwrap(),
                all_day: false,
                location: None,
                mine: true,
                recurring: false,
            },
            std::time::Duration::ZERO,
            &serana_domain::reminder::TimeZoneName::new("Asia/Tbilisi"),
        );
        assert_eq!(topic_of(&booked), Some(Topic::Calendar));
    }

    #[test]
    fn something_that_names_nothing_leaves_the_aim_where_it_was() {
        assert_eq!(topic_of("Which one did you mean?"), None);
        assert_eq!(topic_of(""), None);
    }

    fn fixture() -> serana_domain::reminder::Reminder {
        serana_domain::reminder::Reminder {
            id: serana_domain::reminder::ReminderId::new("a3f9k2xy"),
            owner: serana_domain::reminder::UserId::new(1),
            text: "issue an invoice".into(),
            items: Vec::new(),
            recurrence: serana_domain::reminder::Recurrence::Daily {
                at: jiff::civil::time(9, 0, 0, 0),
            },
            timezone: serana_domain::reminder::TimeZoneName::new("Asia/Tbilisi"),
            created_at: jiff::Timestamp::UNIX_EPOCH,
            next_fire_at: None,
            last_fired_at: None,
            acknowledged_through: None,
        }
    }
}
