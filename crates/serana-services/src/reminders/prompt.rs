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
fn existing(reminders: &[Reminder], now: jiff::Timestamp) -> String {
    if reminders.is_empty() {
        return "none yet".to_owned();
    }
    let rows: Vec<serde_json::Value> = reminders
        .iter()
        .map(|reminder| {
            // Each item carries whether it is ticked *this period*, so "what is left?" is
            // answerable from the context line alone. Without it the model can see the
            // checklist but not its state, and every tick would need a lookup it cannot do.
            let items: Vec<serde_json::Value> = reminder
                .items
                .iter()
                .map(|item| {
                    serde_json::json!({
                        "text": item.text,
                        "done": reminder.is_done(item, now).unwrap_or(false),
                    })
                })
                .collect();
            serde_json::json!({
                "id": reminder.id.as_str(),
                "text": reminder.text,
                "schedule": reminder.recurrence,
                "active": reminder.is_active(),
                // A delivery is a push: it never enters the conversation, so this is the
                // only trace of it. It is what makes "this one is done" resolvable — the
                // reminder that fired most recently is the one being talked about.
                "last_fired_at": reminder.last_fired_at.map(|at| at.to_string()),
                "items": items,
            })
        })
        .collect();
    serde_json::Value::Array(rows).to_string()
}

/// The system prompt. Static, and byte-stable for the life of a conversation.
///
/// Everything that changes between turns — the clock, the reminder list — is deliberately
/// absent. A system prompt that shifts every turn invalidates the provider's prompt cache
/// and re-bills the whole history each time (docs/reference-notes.md §2), so per-turn facts
/// ride the user message instead. See [`context`].
pub(crate) const INSTRUCTIONS: &str = "You manage a person's reminders.\n\
     \n\
     Each message from them begins with a bracketed context line giving the current local \
     time and their reminders as JSON. Read it, then answer what follows it. Resolve \
     relative expressions such as \"tomorrow\", \"next Friday\" or \"in an hour\" against \
     that time.\n\
     \n\
     Choosing what to do:\n\
     - they describe something new -> create_reminder\n\
     - they want an existing one changed -> update_reminder, with its id and the complete \
     new schedule\n\
     - they want one gone for good -> delete_reminder\n\
     - they say they have already done the thing -> acknowledge_reminder, which silences it \
     until the next period instead of deleting it\n\
     - they finished part of a checklist -> complete_items with the lines they mean\n\
     \n\
     They are often vague about which line — \"this one is done\", \"done\", \"finished \
     that\" — because they are answering a reminder that has just arrived. Read the context \
     line: every item says whether it is already done, and last_fired_at says which \
     reminder went off most recently. If exactly one item is still undone, a bare \"done\" \
     means that one. If they say \"the first one\", count the undone items in order. Ask \
     which only when two or more could genuinely be meant.\n\
     \n\
     After ticking anything they want to know what is still outstanding, so never reply \
     with an acknowledgement alone.\n\
     \n\
     A message may begin with \"[replying to this message of yours: ... ]\". That is them \
     pointing at something you sent — very often a reminder that has just arrived. Take it \
     as the subject of what follows, and match it against the context line to find which \
     reminder and which items they mean. Every message you send carries the reminder's id \
     after 🆔 — if the quoted message has one, that IS the reminder, and there is nothing \
     to ask about even when others are worded identically.\n\
     \n\
     Only ever use an id from the context line. If nothing matches what they referred to, \
     or if several match and you cannot tell which they mean, do NOT guess: ask, in one \
     short sentence naming the candidates.\n\
     \n\
     If something you need is missing — most often the time of day, or which days a \
     repeating reminder falls on — ask for just that one thing rather than calling a \
     function with a value you invented. Their next message is the answer, and you will \
     still have this exchange in front of you.\n\
     \n\
     Keep the reminder text in the language the person wrote it in, and keep it in their \
     own words. Strip the scheduling part out of it: \"every month on the 20th, tell me to \
     issue an invoice\" has the text \"issue an invoice\".\n\
     \n\
     When the person lists several things to do rather than one, make it a checklist: put \
     a short heading in the text and each thing as its own item. \"exchange money, \
     transfer to tbc, write to the banker\" is three items, not one line of text. They tick \
     items off as they go, and the reminder falls quiet for the period once the last one \
     is ticked.\n\
     \n\
     An existing reminder can be given a checklist, or have lines added to one, with \
     update_reminder: send its id, its schedule unchanged, and the full list of items you \
     want it to end up with. Lines already there keep their ticks, so repeat them exactly \
     as they appear in the context.\n\
     \n\
     \"turn it into a todo list\", \"make that a checklist\" and the like mean exactly \
     this. When the reminder's own text is already several things run together — \
     \"exchange money, transfer to tbc, write to the banker\" — split it: each becomes one \
     item, and the text becomes a short heading for them, such as \"monthly payment\". \
     Do not leave the list sitting in the text.\n\
     \n\
     A weekly or monthly reminder takes a LIST of days. Say exactly which days it fires \
     on:\n\
     - a single day is a list of one: \"on the 20th\" is [20]\n\
     - write a range out in full: \"from the 20th to the 26th\" is \
     [20, 21, 22, 23, 24, 25, 26]\n\
     - days need not be adjacent: \"on the 20th, and 5 days before\" is [15, 20], and \
     \"on the 1st and the 15th\" is [1, 15]";

/// The per-turn facts, prefixed to what the user actually wrote.
///
/// This is the sanctioned injection point: the system prompt stays untouched, and anything
/// that has to reach the model mid-conversation rides a user message.
pub(crate) fn context(
    local: &jiff::Zoned,
    timezone: &TimeZoneName,
    reminders: &[Reminder],
    now: jiff::Timestamp,
) -> String {
    format!(
        "[now: {date} {time} {weekday}, {zone} | reminders: {existing}]",
        date = local.date(),
        time = local.time().strftime("%H:%M"),
        weekday = local.strftime("%A"),
        zone = timezone,
        existing = existing(reminders, now),
    )
}

/// What to ask the model for when a conversation has grown too long to keep re-sending.
pub(crate) const SUMMARISE: &str = "Summarise the conversation below so it can stand in for \
     the full history. Keep anything still unresolved — a question you asked that has not \
     been answered, a reminder being discussed — and the decisions already made. Drop \
     pleasantries. Write it as notes, in a few sentences, addressed to yourself.";

#[cfg(test)]
mod tests {
    use serana_domain::reminder::{MonthDays, Recurrence, Reminder, ReminderId, TodoItem, UserId};

    use super::*;

    fn ts(raw: &str) -> jiff::Timestamp {
        raw.parse().unwrap()
    }

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
            items: Vec::new(),
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

    fn zone() -> TimeZoneName {
        TimeZoneName::new("Asia/Tbilisi")
    }

    #[test]
    fn the_context_line_carries_the_local_moment() {
        // Without it, "tomorrow" cannot be resolved at all.
        let line = context(
            &at("2026-03-10T06:00:00Z"),
            &zone(),
            &[],
            ts("2026-03-10T06:00:00Z"),
        );
        assert!(line.contains("2026-03-10"), "{line}");
        assert!(line.contains("10:00"), "{line}");
        assert!(line.contains("Tuesday"), "{line}");
        assert!(line.contains("Asia/Tbilisi"), "{line}");
    }

    #[test]
    fn existing_reminders_reach_the_model_with_their_ids() {
        // This is what makes "remove the salary invoice one" resolvable.
        let line = context(
            &at("2026-03-10T06:00:00Z"),
            &zone(),
            &[
                reminder("31k8v7wt", "send salary invoice"),
                reminder("7g2pq0aa", "pay rent"),
            ],
            ts("2026-03-10T06:00:00Z"),
        );
        assert!(line.contains("31k8v7wt"), "{line}");
        assert!(line.contains("send salary invoice"), "{line}");
        assert!(line.contains("7g2pq0aa"), "{line}");
        assert!(line.contains("\"kind\":\"monthly\""), "{line}");
    }

    #[test]
    fn the_context_line_says_which_items_are_already_ticked() {
        // Without this the model can see the checklist but not its state, so "what is
        // left?" and "this one is done" are both unanswerable.
        let mut r = reminder("abc", "monthly payment");
        r.items = vec![
            TodoItem {
                text: "exchange money".into(),
                done_at: Some(ts("2026-03-10T05:00:00Z")),
            },
            TodoItem::new("transfer to tbc"),
        ];
        let line = context(
            &at("2026-03-10T06:00:00Z"),
            &zone(),
            std::slice::from_ref(&r),
            ts("2026-03-10T06:00:00Z"),
        );

        // The whole context line is itself bracketed, so pick out the reminders array
        // rather than the first `[`.
        let parsed: serde_json::Value = {
            let marker = "reminders: ";
            let start = line.find(marker).unwrap() + marker.len();
            serde_json::from_str(line[start..line.len() - 1].trim()).unwrap()
        };
        let items = &parsed[0]["items"];
        assert_eq!(items[0]["text"], "exchange money");
        assert_eq!(items[0]["done"], true);
        assert_eq!(items[1]["text"], "transfer to tbc");
        assert_eq!(items[1]["done"], false);
    }

    #[test]
    fn the_context_line_says_when_a_reminder_last_fired() {
        // A delivery is a push and never enters the conversation, so this is the only
        // trace of it — and what makes "this one is done" resolvable.
        let mut r = reminder("abc", "monthly payment");
        r.last_fired_at = Some(ts("2026-03-10T05:00:00Z"));
        let line = context(
            &at("2026-03-10T06:00:00Z"),
            &zone(),
            std::slice::from_ref(&r),
            ts("2026-03-10T06:00:00Z"),
        );
        assert!(line.contains("last_fired_at"), "{line}");
        assert!(line.contains("2026-03-10T05:00:00Z"), "{line}");
    }

    #[test]
    fn the_context_line_is_one_line_so_it_cannot_be_mistaken_for_the_request() {
        let line = context(
            &at("2026-03-10T06:00:00Z"),
            &zone(),
            &[reminder("a", "one")],
            ts("2026-03-10T06:00:00Z"),
        );
        assert_eq!(line.lines().count(), 1, "{line}");
        assert!(line.starts_with('[') && line.ends_with(']'), "{line}");
    }

    #[test]
    fn an_empty_list_says_so_rather_than_showing_an_empty_array() {
        // `[]` invites the model to treat an id as optional; words do not.
        let line = context(
            &at("2026-03-10T06:00:00Z"),
            &zone(),
            &[],
            ts("2026-03-10T06:00:00Z"),
        );
        assert!(line.contains("none yet"), "{line}");
    }

    #[test]
    fn the_instructions_hold_nothing_that_changes_between_turns() {
        // The prompt-cache invariant, asserted rather than trusted: a date, a time or an
        // id in here would mean a different system prompt on every single turn.
        let today = jiff::Timestamp::now()
            .to_zoned(jiff::tz::TimeZone::get("Asia/Tbilisi").unwrap())
            .date()
            .to_string();
        assert!(!INSTRUCTIONS.contains(&today), "{INSTRUCTIONS}");
        for digit_run in ["2026-", "2027-", ":00,"] {
            assert!(
                !INSTRUCTIONS.contains(digit_run),
                "{digit_run} in the instructions"
            );
        }
    }

    #[test]
    fn the_model_is_told_to_ask_rather_than_guess() {
        assert!(INSTRUCTIONS.contains("do NOT guess"), "{INSTRUCTIONS}");
        assert!(
            INSTRUCTIONS.contains("ask for just that one thing"),
            "{INSTRUCTIONS}"
        );
    }
}
