//! Configuration every frontend needs, read once at startup from the environment.
//!
//! Transport-specific settings — a bot token, an allowlist — stay with their frontend.
//! Everything here is shared, so the CLI and the bot cannot end up pointed at different
//! models or different databases.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, bail};
use serana_domain::reminder::TimeZoneName;

/// Where reminders live inside the container. Overridden by `SERANA_DATA_DIR`.
const DEFAULT_DATA_DIR: &str = "/data";
const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
const DEFAULT_MODEL: &str = "gpt-5";
const DEFAULT_TIMEZONE: &str = "Asia/Tbilisi";
/// How often the scheduler looks for due reminders. A minute is well under the resolution
/// anyone schedules a reminder at, and costs one indexed query.
const DEFAULT_TICK_SECONDS: u64 = 60;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    /// A cheaper model for side work nobody reads — compaction summaries today. `None`
    /// means use [`Self::model`].
    pub auxiliary_model: Option<String>,
    pub timezone: TimeZoneName,
    pub data_dir: PathBuf,
    pub tick_interval: Duration,
    /// Sampling temperature for extraction calls, or `None` for the provider's default.
    pub temperature: Option<f32>,
}

impl AppConfig {
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

        // Unset by default: reasoning models reject every value but their own default and
        // fail the request outright, so sending nothing is the only portable choice.
        let temperature = match std::env::var("SERANA_TEMPERATURE") {
            Ok(raw) if !raw.trim().is_empty() => Some(
                raw.trim()
                    .parse::<f32>()
                    .context("SERANA_TEMPERATURE must be a number, or unset")?,
            ),
            _ => None,
        };

        Ok(Self {
            api_key,
            base_url: var_or("SERANA_BASE_URL", DEFAULT_BASE_URL),
            model: var_or("SERANA_MODEL", DEFAULT_MODEL),
            auxiliary_model: std::env::var("SERANA_AUXILIARY_MODEL")
                .ok()
                .map(|raw| raw.trim().to_owned())
                .filter(|raw| !raw.is_empty()),
            timezone,
            data_dir: PathBuf::from(var_or("SERANA_DATA_DIR", DEFAULT_DATA_DIR)),
            tick_interval: Duration::from_secs(tick_seconds),
            temperature,
        })
    }

    pub fn database_path(&self) -> PathBuf {
        self.data_dir.join("serana.db")
    }
}

pub fn var_or(key: &str, fallback: &str) -> String {
    match std::env::var(key) {
        Ok(value) if !value.trim().is_empty() => value.trim().to_owned(),
        _ => fallback.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> AppConfig {
        AppConfig {
            api_key: "k".into(),
            base_url: DEFAULT_BASE_URL.into(),
            model: DEFAULT_MODEL.into(),
            auxiliary_model: None,
            timezone: TimeZoneName::new(DEFAULT_TIMEZONE),
            data_dir: PathBuf::from("/data"),
            tick_interval: Duration::from_secs(60),
            temperature: None,
        }
    }

    #[test]
    fn the_database_sits_inside_the_data_directory() {
        assert_eq!(config().database_path(), PathBuf::from("/data/serana.db"));
    }

    #[test]
    fn the_default_time_zone_resolves() {
        // A typo here would only surface on the first reminder, hours after deploying.
        assert!(TimeZoneName::new(DEFAULT_TIMEZONE).resolve().is_ok());
    }

    #[test]
    fn a_blank_variable_falls_back_instead_of_yielding_an_empty_string() {
        // An env file written as `SERANA_MODEL=` must not configure an empty model name.
        let key = "SERANA_TEST_BLANK_VAR";
        for value in ["", "   "] {
            unsafe { std::env::set_var(key, value) };
            assert_eq!(var_or(key, "fallback"), "fallback");
        }
        unsafe { std::env::set_var(key, "  actual  ") };
        assert_eq!(var_or(key, "fallback"), "actual", "values are trimmed");
        unsafe { std::env::remove_var(key) };
    }

    #[test]
    fn an_unset_variable_falls_back() {
        assert_eq!(
            var_or("SERANA_TEST_DEFINITELY_UNSET", "fallback"),
            "fallback"
        );
    }
}
