//! Prices from DefiLlama's keyless coins API.
//!
//! The fallback. It takes the same CoinGecko ids, has no documented rate limit and carries
//! the ticker in its answer — but reports no 24-hour move, so a digest served from here is
//! prices only. That is a better daily message than none.

use std::time::Duration;

use async_trait::async_trait;
use serana_domain::prices::{AssetId, PriceError, PriceSource, Quote};

use super::wire::{LlamaCoins, symbol_for};

const API: &str = "https://coins.llama.fi";

#[derive(Debug, Clone)]
pub struct DefiLlamaConfig {
    pub request_timeout: Duration,
    /// Overridden in tests to point at a mock server.
    pub api_base: String,
}

impl Default for DefiLlamaConfig {
    fn default() -> Self {
        Self {
            request_timeout: Duration::from_secs(15),
            api_base: API.to_owned(),
        }
    }
}

/// DefiLlama, behind the domain's port.
pub struct DefiLlama {
    http: reqwest::Client,
    config: DefiLlamaConfig,
}

impl DefiLlama {
    pub fn new(config: DefiLlamaConfig) -> Result<Self, PriceError> {
        let http = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .build()
            .map_err(|e| PriceError::new(e.to_string()))?;
        Ok(Self { http, config })
    }
}

#[async_trait]
impl PriceSource for DefiLlama {
    fn name(&self) -> &str {
        "DefiLlama"
    }

    async fn quotes(&self, assets: &[AssetId]) -> Result<Vec<Quote>, PriceError> {
        if assets.is_empty() {
            return Ok(Vec::new());
        }
        // Ids are namespaced by where they came from; ours are CoinGecko's.
        let keys: Vec<String> = assets
            .iter()
            .map(|asset| format!("coingecko:{asset}"))
            .collect();

        let response = self
            .http
            .get(format!(
                "{}/prices/current/{}",
                self.config.api_base.trim_end_matches('/'),
                keys.join(",")
            ))
            .send()
            .await
            .map_err(|e| PriceError::new(e.to_string()))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| PriceError::new(e.to_string()))?;
        if !status.is_success() {
            return Err(PriceError::new(format!(
                "{}: {}",
                status.as_u16(),
                body.chars().take(160).collect::<String>()
            )));
        }

        let coins: LlamaCoins = serde_json::from_str(&body)
            .map_err(|e| PriceError::new(format!("unreadable prices: {e}")))?;

        Ok(assets
            .iter()
            .zip(keys.iter())
            .filter_map(|(asset, key)| {
                let coin = coins.coins.get(key)?;
                Some(Quote {
                    asset: asset.clone(),
                    symbol: coin.symbol.clone().unwrap_or_else(|| symbol_for(asset)),
                    usd: coin.price?,
                    // Not reported on this route. `None`, never zero: a flat market and an
                    // unknown one must not render the same.
                    change_24h: None,
                })
            })
            .collect())
    }
}
