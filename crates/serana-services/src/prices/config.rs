//! What a price digest is configured with.

use std::time::Duration;

use serana_domain::prices::AssetId;
use serana_domain::reminder::{Recurrence, TimeZoneName};

/// A day. What is cached is considered good until the next scheduled run should have
/// replaced it — so "stale" means a run was missed, not merely that hours have passed.
pub const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// The hour a digest lands on when nobody has said otherwise. Early enough to be the first
/// thing read, late enough not to be a notification in the night.
const DEFAULT_HOUR: i8 = 9;

#[derive(Debug, Clone)]
pub struct PriceConfig {
    /// What to quote, in the order it is shown.
    pub assets: Vec<AssetId>,
    /// The zone the schedule is read in.
    pub timezone: TimeZoneName,
    /// How the digest repeats until the person changes it.
    pub recurrence: Recurrence,
    /// How old what is cached may be before asking reads it again.
    pub cache_ttl: Duration,
}

impl PriceConfig {
    /// The standing order a person gets before they have said anything about it: these
    /// assets, every morning, in their zone.
    pub fn daily(assets: Vec<AssetId>, timezone: TimeZoneName) -> Self {
        Self {
            assets,
            timezone,
            recurrence: Recurrence::Daily {
                at: jiff::civil::time(DEFAULT_HOUR, 0, 0, 0),
            },
            cache_ttl: DEFAULT_CACHE_TTL,
        }
    }
}
