//! Telegram-specific configuration. Everything else lives in [`serana_app::AppConfig`].

use std::collections::HashSet;

use anyhow::Context;
use serana_app::config::var_or;
use serana_domain::reminder::UserId;

#[derive(Debug, Clone)]
pub struct TelegramConfig {
    pub token: String,
    /// Who the bot answers. Empty means nobody — see [`parse_allowed_users`].
    pub allowed_users: HashSet<UserId>,
}

impl TelegramConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            token: std::env::var("TELEGRAM_BOT_TOKEN").context("TELEGRAM_BOT_TOKEN is not set")?,
            allowed_users: parse_allowed_users(&var_or("SERANA_ALLOWED_USER_IDS", ""))?,
        })
    }

    /// Whether the bot talks to this user.
    pub fn allows(&self, user: UserId) -> bool {
        self.allowed_users.contains(&user)
    }
}

/// Parse a comma-separated list of Telegram user ids.
///
/// An empty list means the bot answers nobody. That is deliberate: a personal assistant
/// with a leaked bot token and an open allowlist would take instructions, and spend the
/// owner's tokens, for anyone who found it. Failing closed makes the misconfiguration
/// obvious instead of expensive.
///
/// Entries must be positive: the allowlist is only ever checked against `message.from`,
/// which is a user id and always positive, and the bot has refused every non-private chat
/// since admission was introduced. A zero or negative entry — a group or channel id — can
/// never match, so accepting it silently would leave an owner who added a group expecting
/// the bot to serve it staring at silence with nothing telling them why.
pub fn parse_allowed_users(raw: &str) -> anyhow::Result<HashSet<UserId>> {
    raw.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let id = entry
                .parse::<i64>()
                // Naming the variable is the whole diagnosis: the value alone leaves
                // someone reading a crash loop to guess which line of `.env` is wrong.
                .with_context(|| {
                    format!("SERANA_ALLOWED_USER_IDS: {entry:?} is not a Telegram user id")
                })?;
            anyhow::ensure!(
                id > 0,
                "SERANA_ALLOWED_USER_IDS: {entry:?} looks like a group or channel chat id, \
                 not a user id — the bot only answers commands in a private chat, so it can \
                 never match one"
            );
            Ok(UserId::new(id))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bad_entry_names_the_variable_that_holds_it() {
        // This surfaces as a crash loop at startup, so the message is the only thing the
        // person has to go on — it has to say which line of `.env` to look at.
        let error = parse_allowed_users("notanid").unwrap_err();
        let rendered = format!("{error:#}");
        assert!(rendered.contains("SERANA_ALLOWED_USER_IDS"), "{rendered}");
        assert!(rendered.contains("notanid"), "{rendered}");
    }

    #[test]
    fn an_empty_allowlist_admits_nobody() {
        for raw in ["", "   ", ",", " , , "] {
            assert!(parse_allowed_users(raw).unwrap().is_empty(), "{raw:?}");
        }
        let config = TelegramConfig {
            token: "t".into(),
            allowed_users: HashSet::new(),
        };
        assert!(!config.allows(UserId::new(42)));
    }

    #[test]
    fn ids_are_parsed_and_whitespace_around_them_ignored() {
        let allowed = parse_allowed_users(" 42, 7 ,100 ").unwrap();
        assert_eq!(allowed.len(), 3);
        for id in [42, 7, 100] {
            assert!(allowed.contains(&UserId::new(id)), "{id} should be allowed");
        }
    }

    #[test]
    fn duplicates_collapse() {
        assert_eq!(parse_allowed_users("42,42,42").unwrap().len(), 1);
    }

    #[test]
    fn a_malformed_entry_fails_loudly_rather_than_being_skipped() {
        // Silently dropping it would lock the owner out with no explanation.
        for raw in ["42,nope", "@username", "4 2", "42.0"] {
            assert!(
                parse_allowed_users(raw).is_err(),
                "{raw:?} should be refused"
            );
        }
    }

    #[test]
    fn negative_or_zero_ids_are_refused_as_chat_ids_not_user_ids() {
        // The allowlist is only ever checked against `message.from`, a user id, and the
        // bot refuses every non-private chat outright — so a group or channel id here can
        // never match anything, and an owner who added one deserves an error, not silence.
        for raw in ["-1001234567890", "0"] {
            let error = parse_allowed_users(raw).unwrap_err();
            let rendered = format!("{error:#}");
            assert!(rendered.contains("SERANA_ALLOWED_USER_IDS"), "{rendered}");
        }
    }

    #[test]
    fn an_allowed_user_is_recognised() {
        let config = TelegramConfig {
            token: "t".into(),
            allowed_users: parse_allowed_users("42").unwrap(),
        };
        assert!(config.allows(UserId::new(42)));
        assert!(!config.allows(UserId::new(7)));
    }
}
