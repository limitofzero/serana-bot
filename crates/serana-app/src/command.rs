//! The command surface, shared by every frontend.
//!
//! Transport-agnostic on purpose: Telegram derives its own enum for teloxide's dispatcher
//! and converts into this one, and the CLI parses a line of text into it. Neither owns the
//! command set, so the two frontends cannot drift apart.

/// Something the user asked for.
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
    /// Summarise the conversation so far and carry on from the summary. The history is
    /// what lets a follow-up answer land, so this folds it up rather than discarding it.
    Compact,
}

#[cfg(test)]
mod tests {
    use super::*;

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
