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
pub fn parse_allowed_users(raw: &str) -> anyhow::Result<HashSet<UserId>> {
    raw.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            entry
                .parse::<i64>()
                .map(UserId::new)
                .with_context(|| format!("{entry:?} is not a Telegram user id"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn negative_ids_are_accepted_because_group_chats_have_them() {
        let allowed = parse_allowed_users("-1001234567890").unwrap();
        assert!(allowed.contains(&UserId::new(-1_001_234_567_890)));
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
