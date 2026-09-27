//! What the price APIs send, and the little translation each one needs.

use std::collections::HashMap;

use serde::Deserialize;

use serana_domain::prices::AssetId;

/// One row of CoinGecko's `coins/markets`.
///
/// That route rather than the smaller `simple/price` because it carries the ticker. Without
/// it a digest reads "cow-protocol $0.16", and the alternative — a table of ids to tickers
/// kept here — is a second thing to maintain every time an asset is added.
#[derive(Debug, Deserialize)]
pub(super) struct MarketRow {
    pub id: String,
    pub symbol: Option<String>,
    /// Absent when CoinGecko knows the asset but has no USD price for it.
    pub current_price: Option<f64>,
    pub price_change_percentage_24h: Option<f64>,
}

/// CoinGecko's `search`: coins, alongside exchanges and categories we do not want.
#[derive(Debug, Deserialize)]
pub(super) struct SearchResults {
    #[serde(default)]
    pub coins: Vec<SearchCoin>,
}

#[derive(Debug, Deserialize)]
pub(super) struct SearchCoin {
    pub id: String,
    pub name: String,
    pub symbol: String,
    pub market_cap_rank: Option<u32>,
}

/// DefiLlama's `prices/current`: an object keyed by `source:id`.
#[derive(Debug, Deserialize)]
pub(super) struct LlamaCoins {
    #[serde(default)]
    pub coins: HashMap<String, LlamaCoin>,
}

#[derive(Debug, Deserialize)]
pub(super) struct LlamaCoin {
    pub price: Option<f64>,
    pub symbol: Option<String>,
}

/// A ticker for an asset whose source did not give one.
///
/// Both sources normally do. This is the last resort, and the id upper-cased is readable
/// even when it is not the real ticker — which beats showing nothing.
pub(super) fn symbol_for(asset: &AssetId) -> String {
    asset.as_str().to_uppercase()
}
