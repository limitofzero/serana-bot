//! The command surface, shared by every frontend.
//!
//! Transport-agnostic on purpose: Telegram derives its own enum for teloxide's dispatcher
//! and converts into this one, and the CLI parses a line of text into it. Neither owns the
//! command set, so the two frontends cannot drift apart.

/// Something the user asked for.
/// How much of a quoted message is worth carrying. A reply to a long checklist should not
/// double the size of every turn.
const QUOTE_LIMIT: usize = 280;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Explain what the assistant can do.
    Help,
    /// Anything to do with reminders, in the user's own words: setting one, changing one,
    /// removing one, or saying they have dealt with one. The model reads the intent, so
    /// there is deliberately no command per verb and no id to type by hand.
    Reminder(String),
    /// List the user's reminders. Kept separate from [`Command::Reminder`] because it costs
    /// no model call and cannot be misread — which is exactly what you want when the model
    /// has just misunderstood something.
    Reminders,
    /// Anything to do with the calendar, in the user's own words: what is on, putting
    /// something on it, calling something off. Like [`Command::Reminder`], the model reads
    /// the intent, so there is no command per verb and no id to type by hand.
    Calendar(String),
    /// Summarise the conversation so far and carry on from the summary. The history is
    /// what lets a follow-up answer land, so this folds it up rather than discarding it.
    Compact,
}

/// What a message that is not a command is about.
///
/// The assistant keeps one conversation per subject, so a bare "yes" has to be aimed
/// somewhere. It is aimed at whatever the person was last talking about — which is the only
/// reading that lets the calendar ask "delete this for good?" and get an answer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Topic {
    #[default]
    Reminders,
    Calendar,
}

impl Command {
    /// A bare message, continuing whatever was last being talked about.
    pub fn bare(topic: Topic, request: &str) -> Self {
        let request = request.trim().to_owned();
        match topic {
            Topic::Reminders => Self::Reminder(request),
            Topic::Calendar => Self::Calendar(request),
        }
    }

    /// A request made as a reply to an earlier message.
    ///
    /// The quoted text is folded into the request rather than carried beside it, because
    /// that is where the model needs it: "this one is done" is only answerable if what was
    /// replied to is in front of it. A push from the scheduler never enters the
    /// conversation, so replying to a delivered checklist is otherwise the one gesture the
    /// assistant cannot follow.
    pub fn replying(topic: Topic, quoted: &str, request: &str) -> Self {
        let quoted = quoted.trim();
        if quoted.is_empty() {
            return Self::bare(topic, request);
        }
        let mut quoted: String = quoted.chars().take(QUOTE_LIMIT).collect();
        if quoted.chars().count() == QUOTE_LIMIT {
            quoted.push('…');
        }
        Self::bare(
            topic,
            &format!(
                "[replying to this message of yours:\n{quoted}\n]\n{}",
                request.trim()
            ),
        )
    }

    /// Which subject this command is about, when it is about one.
    ///
    /// `/help` and `/compact` are about neither, and must not move the aim of the next bare
    /// message: asking for help in the middle of arranging something does not end that.
    pub fn topic(&self) -> Option<Topic> {
        match self {
            Self::Reminder(_) | Self::Reminders => Some(Topic::Reminders),
            Self::Calendar(_) => Some(Topic::Calendar),
            Self::Help | Self::Compact => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_carries_what_it_replied_to() {
        let Command::Reminder(request) = Command::replying(
            Topic::Reminders,
            "⏰ mortgage\n⚪ exchange money\n⚪ transfer to tbc",
            "first one is done",
        ) else {
            panic!("a reply is a reminder request");
        };
        assert!(request.contains("exchange money"), "{request}");
        assert!(request.ends_with("first one is done"), "{request}");
    }

    #[test]
    fn a_reply_to_nothing_readable_is_an_ordinary_request() {
        assert_eq!(
            Command::replying(Topic::Reminders, "   ", "  set a reminder  "),
            Command::Reminder("set a reminder".into())
        );
    }

    #[test]
    fn a_long_quote_is_trimmed_rather_than_riding_along_in_full() {
        // It is re-sent with the turn; an unbounded quote is an unbounded bill.
        let huge = "x".repeat(5_000);
        let Command::Reminder(request) = Command::replying(Topic::Reminders, &huge, "done") else {
            unreachable!()
        };
        assert!(request.chars().count() < 400, "{}", request.chars().count());
        assert!(request.contains('…'), "and says it was cut");
    }

    #[test]
    fn a_reply_about_the_calendar_stays_with_the_calendar() {
        // The calendar asks "delete this for good?" and the answer is the next message. If
        // it landed on reminders the question could never be answered at all.
        assert_eq!(
            Command::replying(Topic::Calendar, "🗑 Delete the dentist?", "yes"),
            Command::Calendar(
                "[replying to this message of yours:\n🗑 Delete the dentist?\n]\nyes".into()
            )
        );
    }

    #[test]
    fn help_and_compact_do_not_move_what_a_bare_message_continues() {
        // Asking for help in the middle of arranging something does not end it.
        assert_eq!(Command::Help.topic(), None);
        assert_eq!(Command::Compact.topic(), None);
        assert_eq!(Command::Calendar("x".into()).topic(), Some(Topic::Calendar));
        assert_eq!(Command::Reminders.topic(), Some(Topic::Reminders));
    }

    #[test]
    fn commands_carrying_arguments_keep_them_verbatim() {
        // The argument is the user's phrasing; trimming or lowercasing it here would change
        // what the model is asked to read.
        let raw = "каждый месяц 20 число - оформить Invoice";
        assert_eq!(
            Command::Reminder(raw.into()),
            Command::Reminder(raw.to_owned())
        );
    }
}
