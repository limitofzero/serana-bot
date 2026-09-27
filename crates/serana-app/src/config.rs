//! Configuration every frontend needs, read once at startup from the environment.
//!
//! Transport-specific settings — a bot token, an allowlist — stay with their frontend.
//! Everything here is shared, so the CLI and the bot cannot end up pointed at different
//! models or different databases.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, bail};
use serana_adapters::{CoinGeckoConfig, GoogleCalendarConfig};
use serana_domain::prices::AssetId;
use serana_domain::reminder::TimeZoneName;

/// Where reminders live inside the container. Overridden by `SERANA_DATA_DIR`.
const DEFAULT_DATA_DIR: &str = "/data";
const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
const DEFAULT_MODEL: &str = "gpt-5";
const DEFAULT_TIMEZONE: &str = "Asia/Tbilisi";
/// How often the scheduler looks for due reminders. A minute is well under the resolution
/// anyone schedules a reminder at, and costs one indexed query.
const DEFAULT_TICK_SECONDS: u64 = 60;
/// How long to reserve either side of an appointment the person has to travel to. Half an
/// hour is a city's worth of getting there; `SERANA_TRAVEL_MINUTES` overrides it.
const DEFAULT_TRAVEL_MINUTES: u64 = 30;
/// What the price digest quotes when nobody has said otherwise. CoinGecko ids, because
/// that is what both price sources take.
const DEFAULT_ASSETS: &str = "bitcoin,ethereum,cow-protocol";

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
    /// Google Calendar, or `None` when nothing is configured. Absent is a supported way to
    /// run: reminders do not need one, and the assistant says so rather than failing.
    pub calendar: Option<GoogleCalendarConfig>,
    /// Reserved either side of an in-person appointment.
    pub travel: Duration,
    /// What the price digest quotes, in the order it is shown.
    pub assets: Vec<AssetId>,
    /// CoinGecko. The keyless route is the default; a demo key only lifts the rate limit.
    pub coingecko: CoinGeckoConfig,
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

        let travel_minutes = var_or("SERANA_TRAVEL_MINUTES", &DEFAULT_TRAVEL_MINUTES.to_string())
            .parse::<u64>()
            .context("SERANA_TRAVEL_MINUTES must be a whole number of minutes")?;

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
            calendar: google_from_env(),
            travel: Duration::from_secs(travel_minutes * 60),
            assets: parse_assets(&var_or("SERANA_PRICE_ASSETS", DEFAULT_ASSETS)),
            coingecko: CoinGeckoConfig {
                api_key: var_or("SERANA_COINGECKO_API_KEY", ""),
                ..CoinGeckoConfig::default()
            },
        })
    }

    pub fn database_path(&self) -> PathBuf {
        self.data_dir.join("serana.db")
    }
}

/// Google Calendar from the environment, or `None` when the three secrets are not all
/// there.
///
/// Partial configuration is treated as none rather than as an error: a half-filled `.env`
/// is how someone is left with a bot that will not start, and there is nothing here that
/// reminders need.
fn google_from_env() -> Option<GoogleCalendarConfig> {
    let config = GoogleCalendarConfig {
        client_id: var_or("SERANA_GOOGLE_CLIENT_ID", ""),
        client_secret: var_or("SERANA_GOOGLE_CLIENT_SECRET", ""),
        refresh_token: var_or("SERANA_GOOGLE_REFRESH_TOKEN", ""),
        calendar_id: var_or("SERANA_GOOGLE_CALENDAR_ID", "primary"),
        ..GoogleCalendarConfig::default()
    };
    config.is_configured().then_some(config)
}

/// A comma-separated watchlist.
///
/// Blank entries are dropped rather than refused: `bitcoin,,ethereum` is a trailing comma
/// somebody left behind, not a request to price the empty string.
fn parse_assets(raw: &str) -> Vec<AssetId> {
    raw.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(AssetId::new)
        .collect()
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
            calendar: None,
            travel: Duration::from_secs(30 * 60),
            assets: parse_assets(DEFAULT_ASSETS),
            coingecko: CoinGeckoConfig::default(),
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
    fn a_half_filled_google_block_configures_no_calendar_rather_than_a_broken_one() {
        // A `.env` with the id pasted in and the token still to come is the normal state
        // halfway through setting this up. It must not stop the bot starting.
        let keys = [
            "SERANA_GOOGLE_CLIENT_ID",
            "SERANA_GOOGLE_CLIENT_SECRET",
            "SERANA_GOOGLE_REFRESH_TOKEN",
        ];
        for key in keys {
            unsafe { std::env::remove_var(key) };
        }
        assert!(google_from_env().is_none(), "nothing set");

        unsafe { std::env::set_var("SERANA_GOOGLE_CLIENT_ID", "id") };
        assert!(google_from_env().is_none(), "only the id");

        unsafe { std::env::set_var("SERANA_GOOGLE_CLIENT_SECRET", "secret") };
        unsafe { std::env::set_var("SERANA_GOOGLE_REFRESH_TOKEN", "token") };
        let configured = google_from_env().expect("all three");
        assert_eq!(configured.calendar_id, "primary", "the default calendar");

        for key in keys {
            unsafe { std::env::remove_var(key) };
        }
    }

    #[test]
    fn the_default_watchlist_is_the_three_that_were_asked_for() {
        assert_eq!(
            parse_assets(DEFAULT_ASSETS)
                .iter()
                .map(|a| a.as_str().to_owned())
                .collect::<Vec<_>>(),
            vec!["bitcoin", "ethereum", "cow-protocol"]
        );
    }

    #[test]
    fn a_watchlist_survives_the_punctuation_people_leave_behind() {
        // A trailing comma is not a request to price the empty string.
        assert_eq!(
            parse_assets(" bitcoin , ,ethereum,").len(),
            2,
            "blank entries are dropped"
        );
        assert!(parse_assets("   ").is_empty());
    }

    #[test]
    fn an_unset_variable_falls_back() {
        assert_eq!(
            var_or("SERANA_TEST_DEFINITELY_UNSET", "fallback"),
            "fallback"
        );
    }
}
