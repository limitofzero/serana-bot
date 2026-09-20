//! Configuration, read once at startup from the environment.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, bail};
use serana_domain::reminder::{TimeZoneName, UserId};

/// Where reminders live inside the container. Overridden by `SERANA_DATA_DIR`.
const DEFAULT_DATA_DIR: &str = "/data";
const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
const DEFAULT_MODEL: &str = "gpt-5";
const DEFAULT_TIMEZONE: &str = "Asia/Tbilisi";
/// How often the scheduler looks for due reminders. A minute is well under the resolution
/// anyone schedules a reminder at, and costs one indexed query.
const DEFAULT_TICK_SECONDS: u64 = 60;

#[derive(Debug, Clone)]
pub struct Config {
    pub telegram_token: String,
    /// Who the bot answers. Empty means nobody — see [`parse_allowed_users`].
    pub allowed_users: HashSet<UserId>,
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub timezone: TimeZoneName,
    pub data_dir: PathBuf,
    pub tick_interval: Duration,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let timezone = TimeZoneName::new(var_or("SERANA_TIMEZONE", DEFAULT_TIMEZONE));
        // Fail at startup rather than when the first reminder is created.
        timezone
            .resolve()
            .with_context(|| format!("SERANA_TIMEZONE={timezone} is not a known time zone"))?;

        let api_key = std::env::var("SERANA_API_KEY").unwrap_or_default();
        if api_key.is_empty() {
            bail!("SERANA_API_KEY is not set; the bot cannot read reminder requests without it");
        }

        let tick_seconds = var_or("SERANA_TICK_SECONDS", &DEFAULT_TICK_SECONDS.to_string())
            .parse::<u64>()
            .context("SERANA_TICK_SECONDS must be a whole number of seconds")?;
        if tick_seconds == 0 {
            bail!("SERANA_TICK_SECONDS must be at least 1");
        }

        Ok(Self {
            telegram_token: std::env::var("TELEGRAM_BOT_TOKEN")
                .context("TELEGRAM_BOT_TOKEN is not set")?,
            allowed_users: parse_allowed_users(&var_or("SERANA_ALLOWED_USER_IDS", ""))?,
            api_key,
            base_url: var_or("SERANA_BASE_URL", DEFAULT_BASE_URL),
            model: var_or("SERANA_MODEL", DEFAULT_MODEL),
            timezone,
            data_dir: PathBuf::from(var_or("SERANA_DATA_DIR", DEFAULT_DATA_DIR)),
            tick_interval: Duration::from_secs(tick_seconds),
        })
    }

    pub fn database_path(&self) -> PathBuf {
        self.data_dir.join("serana.db")
    }

    /// Whether the bot talks to this user.
    pub fn allows(&self, user: UserId) -> bool {
        self.allowed_users.contains(&user)
    }
}

fn var_or(key: &str, fallback: &str) -> String {
    match std::env::var(key) {
        Ok(value) if !value.trim().is_empty() => value.trim().to_owned(),
        _ => fallback.to_owned(),
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
    fn a_single_id_works() {
        assert_eq!(
            parse_allowed_users("42").unwrap(),
            HashSet::from([UserId::new(42)])
        );
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
        assert!(
            parse_allowed_users("-1001234567890")
                .unwrap()
                .contains(&UserId::new(-1_001_234_567_890))
        );
    }

    #[test]
    fn the_database_sits_inside_the_data_directory() {
        let config = Config {
            telegram_token: "t".into(),
            allowed_users: HashSet::new(),
            api_key: "k".into(),
            base_url: DEFAULT_BASE_URL.into(),
            model: DEFAULT_MODEL.into(),
            timezone: TimeZoneName::new(DEFAULT_TIMEZONE),
            data_dir: PathBuf::from("/data"),
            tick_interval: Duration::from_secs(60),
        };
        assert_eq!(config.database_path(), PathBuf::from("/data/serana.db"));
        assert!(
            !config.allows(UserId::new(42)),
            "an empty allowlist admits nobody"
        );
    }

    #[test]
    fn the_default_time_zone_resolves() {
        // A typo here would only surface on the first reminder, hours after deploying.
        assert!(TimeZoneName::new(DEFAULT_TIMEZONE).resolve().is_ok());
    }
}
