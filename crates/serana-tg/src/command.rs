//! Telegram's own command enum, and its translation into the shared one.
//!
//! teloxide's dispatcher needs a type carrying its derive, so the mapping lives here rather
//! than putting a Telegram dependency into `serana-ui`.

use serana_app::Command;
use teloxide::utils::command::BotCommands;

#[derive(BotCommands, Clone, Debug, PartialEq, Eq)]
#[command(rename_rule = "snake_case")]
pub enum TelegramCommand {
    /// Show what this bot can do.
    Help,
    /// Sent automatically when a chat opens; the same as /help.
    Start,
    /// Set, change, remove or finish a reminder — just say what you want.
    Reminder(String),
    /// List your reminders.
    Reminders,
    /// Fold this conversation up into a summary.
    Compact,
}

impl From<TelegramCommand> for Command {
    fn from(command: TelegramCommand) -> Self {
        match command {
            // Telegram sends /start on first contact; there is nothing else useful to say.
            TelegramCommand::Help | TelegramCommand::Start => Self::Help,
            TelegramCommand::Reminder(request) => Self::Reminder(request),
            TelegramCommand::Reminders => Self::Reminders,
            TelegramCommand::Compact => Self::Compact,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_is_treated_as_help() {
        assert_eq!(Command::from(TelegramCommand::Start), Command::Help);
        assert_eq!(Command::from(TelegramCommand::Help), Command::Help);
    }

    #[test]
    fn arguments_cross_the_boundary_untouched() {
        let raw = "каждый месяц 20 число - оформить invoice";
        assert_eq!(
            Command::from(TelegramCommand::Reminder(raw.into())),
            Command::Reminder(raw.into())
        );
    }

    #[test]
    fn the_menu_registered_with_telegram_lists_every_command() {
        // `main` sends this to `setMyCommands` at startup. Telegram stores the menu per
        // token, so a variant missing here stays invisible to users — and a menu left
        // unregistered keeps showing whatever program held the token before.
        let registered: Vec<String> = TelegramCommand::bot_commands()
            .into_iter()
            .map(|command| command.command)
            .collect();
        assert_eq!(
            registered,
            vec!["/help", "/start", "/reminder", "/reminders", "/compact"]
        );
    }

    #[test]
    fn every_registered_command_carries_a_description() {
        // An empty description renders as a blank row in Telegram's menu.
        for command in TelegramCommand::bot_commands() {
            assert!(
                !command.description.trim().is_empty(),
                "{} has no description",
                command.command
            );
        }
    }

    #[test]
    fn the_commands_telegram_advertises_are_the_ones_users_are_told_about() {
        // `descriptions()` is what /help in Telegram's own menu shows.
        let advertised = TelegramCommand::descriptions().to_string();
        for command in ["/reminder", "/reminders", "/help"] {
            assert!(
                advertised.contains(command),
                "{command} is missing from {advertised}"
            );
        }
    }
}
