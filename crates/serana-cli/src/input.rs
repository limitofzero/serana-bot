//! Turning a typed line into a command.
//!
//! The leading slash is optional: it is what a Telegram user's fingers already know, and a
//! terminal user should not have to type it.

use serana_app::Command;

/// What the user typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Command(Command),
    /// Leave the REPL.
    Quit,
    /// Nothing but whitespace; redraw the prompt.
    Blank,
    /// A word that is not a command. Carries it so the reply can name it.
    Unknown(String),
}

pub fn parse(line: &str) -> Input {
    let line = line.trim();
    if line.is_empty() {
        return Input::Blank;
    }

    let line = line.strip_prefix('/').unwrap_or(line);
    let (word, rest) = match line.split_once(char::is_whitespace) {
        Some((word, rest)) => (word, rest.trim()),
        None => (line, ""),
    };

    match word.to_lowercase().as_str() {
        "help" | "h" | "?" => Input::Command(Command::Help),
        "reminder" => Input::Command(Command::Reminder(rest.to_owned())),
        "reminders" => Input::Command(Command::Reminders),
        "quit" | "exit" | "q" => Input::Quit,
        other => Input::Unknown(other.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slash_is_optional() {
        assert_eq!(parse("/reminders"), Input::Command(Command::Reminders));
        assert_eq!(parse("reminders"), Input::Command(Command::Reminders));
    }

    #[test]
    fn the_argument_is_everything_after_the_command() {
        let request = "каждый месяц 20 число - оформить invoice";
        assert_eq!(
            parse(&format!("/reminder {request}")),
            Input::Command(Command::Reminder(request.to_owned()))
        );
    }

    #[test]
    fn the_argument_keeps_its_internal_punctuation_and_case() {
        // It is the user's phrasing, and it is what the model will read.
        let request = "Каждую Пятницу в 18:00 — отчёт!";
        assert_eq!(
            parse(&format!("reminder {request}")),
            Input::Command(Command::Reminder(request.to_owned()))
        );
    }

    #[test]
    fn surrounding_whitespace_is_ignored() {
        assert_eq!(
            parse("   /reminders   "),
            Input::Command(Command::Reminders)
        );
        assert_eq!(
            parse("  reminder   каждый день  "),
            Input::Command(Command::Reminder("каждый день".to_owned()))
        );
    }

    #[test]
    fn a_command_without_its_argument_yields_an_empty_one() {
        // The handler explains what is missing; the parser does not second-guess it.
        assert_eq!(
            parse("/reminder"),
            Input::Command(Command::Reminder(String::new()))
        );
    }

    #[test]
    fn reminder_and_reminders_are_not_confused() {
        assert_eq!(parse("reminders"), Input::Command(Command::Reminders));
        assert_eq!(
            parse("reminder"),
            Input::Command(Command::Reminder(String::new()))
        );
    }

    #[test]
    fn the_command_word_is_case_insensitive_but_the_argument_is_not() {
        assert_eq!(parse("/REMINDERS"), Input::Command(Command::Reminders));
        assert_eq!(
            parse("/Reminder Оформить Invoice"),
            Input::Command(Command::Reminder("Оформить Invoice".to_owned()))
        );
    }

    #[test]
    fn help_and_quit_have_the_short_forms_people_type() {
        for line in ["help", "/help", "h", "?"] {
            assert_eq!(parse(line), Input::Command(Command::Help), "{line}");
        }
        for line in ["quit", "/quit", "exit", "q"] {
            assert_eq!(parse(line), Input::Quit, "{line}");
        }
    }

    #[test]
    fn removing_and_finishing_go_through_reminder_like_everything_else() {
        // There is no /reminder_delete any more: the model reads the verb, so these are
        // ordinary reminder requests.
        for line in [
            "/reminder remove the invoice one",
            "/reminder done with the invoice",
        ] {
            assert!(
                matches!(parse(line), Input::Command(Command::Reminder(_))),
                "{line}"
            );
        }
        assert_eq!(
            parse("/reminder_delete a3f9k2"),
            Input::Unknown("reminder_delete".to_owned())
        );
    }

    #[test]
    fn blank_lines_are_not_commands() {
        for line in ["", "   ", "\t", "\n", "/"] {
            assert!(
                matches!(parse(line), Input::Blank | Input::Unknown(_)),
                "{line:?} should not be a command"
            );
        }
        assert_eq!(parse("   "), Input::Blank);
    }

    #[test]
    fn an_unknown_word_is_reported_with_the_word() {
        assert_eq!(
            parse("/frobnicate now"),
            Input::Unknown("frobnicate".to_owned())
        );
        assert_eq!(parse("привет"), Input::Unknown("привет".to_owned()));
    }
}
