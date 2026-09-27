//! What a price is, and what a set of them taken together is.

use serde::{Deserialize, Serialize};

/// An asset, named the way the price source names it.
///
/// Opaque on purpose: these are CoinGecko ids (`bitcoin`, `cow-protocol`), and every source
/// worth using accepts them. A source that does not is free to translate.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AssetId(String);

impl AssetId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AssetId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One asset's price.
///
/// `f64` because this is a number to look at, never a number to settle with. Nothing in
/// this system adds prices up, holds a balance or signs a transaction; the only operation
/// is rendering one to a few significant figures. A decimal type here would buy precision
/// that is then thrown away by the formatter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Quote {
    pub asset: AssetId,
    /// The ticker, as the source gives it: `BTC`, `ETH`, `COW`.
    pub symbol: String,
    pub usd: f64,
    /// Percentage move over the last 24 hours. `None` when the source does not report one —
    /// which is not the same as zero, and must not be rendered as "0.0%".
    pub change_24h: Option<f64>,
}

/// Every price, as of one moment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub quotes: Vec<Quote>,
    /// When these were read. The digest shows it, because a price without its moment is a
    /// number nobody can act on.
    pub taken_at: jiff::Timestamp,
    /// Which source answered. Shown when it was not the first choice, so a digest missing
    /// its 24-hour moves explains itself instead of looking broken.
    pub source: String,
}

impl Snapshot {
    pub fn new(quotes: Vec<Quote>, taken_at: jiff::Timestamp, source: impl Into<String>) -> Self {
        Self {
            quotes,
            taken_at,
            source: source.into(),
        }
    }

    /// How old this is at `now`. Saturates at zero: a snapshot stamped in the future is a
    /// clock disagreement between us and the source, not a negative age.
    pub fn age(&self, now: jiff::Timestamp) -> std::time::Duration {
        std::time::Duration::try_from(now - self.taken_at).unwrap_or_default()
    }

    pub fn is_empty(&self) -> bool {
        self.quotes.is_empty()
    }
}
