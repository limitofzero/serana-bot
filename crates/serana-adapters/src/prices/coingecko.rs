//! Prices from CoinGecko's keyless `coins/markets` route.
//!
//! The only free source that gives ticker, price and 24-hour move for every asset in one
//! request, which is exactly a digest's shape. Keyless works and needs no signup, but
//! shares a rate limit across everyone on the same egress IP; a demo key lifts that and is
//! the only reason [`CoinGeckoConfig::api_key`] exists.

use std::time::Duration;

use async_trait::async_trait;
use serana_domain::prices::{AssetId, AssetMatch, PriceError, PriceSource, Quote};

use super::wire::{MarketRow, SearchResults, symbol_for};

const API: &str = "https://api.coingecko.com/api/v3";

#[derive(Debug, Clone)]
pub struct CoinGeckoConfig {
    /// A CoinGecko Demo key, or empty for the keyless route.
    pub api_key: String,
    pub request_timeout: Duration,
    /// Overridden in tests to point at a mock server.
    pub api_base: String,
}

impl Default for CoinGeckoConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            request_timeout: Duration::from_secs(15),
            api_base: API.to_owned(),
        }
    }
}

/// CoinGecko, behind the domain's port.
pub struct CoinGecko {
    http: reqwest::Client,
    config: CoinGeckoConfig,
}

impl CoinGecko {
    pub fn new(config: CoinGeckoConfig) -> Result<Self, PriceError> {
        let http = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .build()
            .map_err(|e| PriceError::new(e.to_string()))?;
        Ok(Self { http, config })
    }

    /// A GET against the API, with the demo key when there is one, returning the body.
    async fn get(&self, route: &str, query: &[(&str, &str)]) -> Result<String, PriceError> {
        let mut request = self
            .http
            .get(format!(
                "{}/{route}",
                self.config.api_base.trim_end_matches('/')
            ))
            .query(query);
        if !self.config.api_key.is_empty() {
            request = request.header("x-cg-demo-api-key", &self.config.api_key);
        }

        let response = request
            .send()
            .await
            .map_err(|e| PriceError::new(e.to_string()))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| PriceError::new(e.to_string()))?;
        if !status.is_success() {
            // 429 is the one worth naming: it is what the keyless route does under load,
            // and it is the reason a fallback exists at all.
            return Err(PriceError::new(if status.as_u16() == 429 {
                "429 (rate limited)".to_owned()
            } else {
                format!(
                    "{}: {}",
                    status.as_u16(),
                    body.chars().take(160).collect::<String>()
                )
            }));
        }
        Ok(body)
    }
}

#[async_trait]
impl PriceSource for CoinGecko {
    fn name(&self) -> &str {
        "CoinGecko"
    }

    async fn quotes(&self, assets: &[AssetId]) -> Result<Vec<Quote>, PriceError> {
        if assets.is_empty() {
            return Ok(Vec::new());
        }
        let ids = assets
            .iter()
            .map(AssetId::as_str)
            .collect::<Vec<_>>()
            .join(",");

        let body = self
            .get(
                "coins/markets",
                &[
                    ("vs_currency", "usd"),
                    ("ids", ids.as_str()),
                    ("price_change_percentage", "24h"),
                ],
            )
            .await?;

        let rows: Vec<MarketRow> = serde_json::from_str(&body)
            .map_err(|e| PriceError::new(format!("unreadable prices: {e}")))?;

        // Re-ordered to what was asked for: CoinGecko answers in its own ranking order, and
        // the digest shows the watchlist in the order the person wrote it. An asset the
        // source did not price is left out rather than reported as free.
        Ok(assets
            .iter()
            .filter_map(|asset| {
                let row = rows.iter().find(|row| row.id == asset.as_str())?;
                Some(Quote {
                    asset: asset.clone(),
                    symbol: row
                        .symbol
                        .as_deref()
                        .map(str::to_uppercase)
                        .unwrap_or_else(|| symbol_for(asset)),
                    usd: row.current_price?,
                    change_24h: row.price_change_percentage_24h,
                })
            })
            .collect())
    }

    async fn search(&self, query: &str) -> Result<Vec<AssetMatch>, PriceError> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let body = self.get("search", &[("query", query.trim())]).await?;
        let results: SearchResults = serde_json::from_str(&body)
            .map_err(|e| PriceError::new(format!("unreadable search results: {e}")))?;
        Ok(results
            .coins
            .into_iter()
            .map(|coin| AssetMatch {
                id: AssetId::new(coin.id),
                name: coin.name,
                symbol: coin.symbol.trim().to_owned(),
                rank: coin.market_cap_rank,
            })
            .collect())
    }
}
