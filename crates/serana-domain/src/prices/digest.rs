//! The standing order: which prices, how often, and the last set that was read.

use serde::{Deserialize, Serialize};

use crate::error::StorageError;
use crate::reminder::{Recurrence, TimeZoneName, UserId};

use super::quote::{AssetId, Snapshot};

/// A person's price digest.
///
/// One per person, keyed by owner — there is no id to type, because there is nothing to
/// disambiguate. The schedule is a [`Recurrence`] rather than a second scheduling model:
/// "every day at 9", "weekdays at 9" and "the 1st of the month" are the same vocabulary a
/// reminder already speaks, and the next-occurrence arithmetic is the part worth reusing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Digest {
    pub owner: UserId,
    pub recurrence: Recurrence,
    pub timezone: TimeZoneName,
    /// What to quote, in the order it is shown.
    pub assets: Vec<AssetId>,
    /// When the next digest goes out. `None` means the digest is switched off.
    ///
    /// Denormalised onto the row so "what is due" is one indexed lookup rather than
    /// evaluating every recurrence on every tick — the same bargain reminders make.
    pub next_fire_at: Option<jiff::Timestamp>,
    pub last_sent_at: Option<jiff::Timestamp>,
    /// The last prices read, kept so asking costs nothing.
    ///
    /// The scheduled run writes it; the command reads it. That is the whole point of a
    /// digest: the numbers are already here when you ask for them.
    #[serde(default)]
    pub cached: Option<Snapshot>,
}

impl Digest {
    pub fn new(
        owner: UserId,
        recurrence: Recurrence,
        timezone: TimeZoneName,
        assets: Vec<AssetId>,
    ) -> Self {
        Self {
            owner,
            recurrence,
            timezone,
            assets,
            next_fire_at: None,
            last_sent_at: None,
            cached: None,
        }
    }

    /// Recompute [`Digest::next_fire_at`] from `now`.
    ///
    /// Called on creation and after every send. Returns the new value.
    pub fn reschedule(
        &mut self,
        now: jiff::Timestamp,
    ) -> Result<Option<jiff::Timestamp>, StorageError> {
        let tz = self.timezone.resolve()?;
        self.next_fire_at = self.recurrence.next_occurrence_after(now, &tz);
        Ok(self.next_fire_at)
    }

    /// Record a delivery and advance the schedule.
    pub fn mark_sent(
        &mut self,
        now: jiff::Timestamp,
        snapshot: Snapshot,
    ) -> Result<Option<jiff::Timestamp>, StorageError> {
        self.last_sent_at = Some(now);
        self.cached = Some(snapshot);
        self.reschedule(now)
    }

    /// Add an asset to the end of the watchlist. Returns whether it was new.
    ///
    /// What is cached is dropped, because it no longer covers what is watched: showing it
    /// would present a list missing the very asset just asked for, and a day would pass
    /// before the next scheduled run noticed.
    pub fn watch(&mut self, asset: AssetId) -> bool {
        if self.assets.contains(&asset) {
            return false;
        }
        self.assets.push(asset);
        self.cached = None;
        true
    }

    /// Take an asset off the watchlist. Returns whether it was there.
    ///
    /// What is cached is kept, less that asset's row: everything else in it is still true,
    /// and reading the market again to show one line fewer would be waste.
    pub fn unwatch(&mut self, asset: &AssetId) -> bool {
        let before = self.assets.len();
        self.assets.retain(|watched| watched != asset);
        if let Some(cached) = &mut self.cached {
            cached.quotes.retain(|quote| &quote.asset != asset);
        }
        self.assets.len() != before
    }

    /// The watched asset someone meant by `name` — its id, or the ticker it was last shown
    /// under. Case does not matter; nobody types "COW" and "cow" as different things.
    pub fn find(&self, name: &str) -> Option<&AssetId> {
        let wanted = name.trim().to_lowercase();
        self.assets
            .iter()
            .find(|asset| asset.as_str().to_lowercase() == wanted)
            .or_else(|| {
                let quote = self
                    .cached
                    .as_ref()?
                    .quotes
                    .iter()
                    .find(|quote| quote.symbol.trim().to_lowercase() == wanted)?;
                self.assets.iter().find(|asset| **asset == quote.asset)
            })
    }

    /// Whether the digest will go out again.
    pub fn is_active(&self) -> bool {
        self.next_fire_at.is_some()
    }

    /// Whether what is cached is too old to show without reading it again.
    ///
    /// Stale means a scheduled run was missed — the process was down, or the source was —
    /// not merely that time has passed. Showing a price from three days ago as though it
    /// were today's is the one thing a price digest must never do.
    pub fn is_stale(&self, now: jiff::Timestamp, ttl: std::time::Duration) -> bool {
        match &self.cached {
            None => true,
            Some(snapshot) => snapshot.age(now) > ttl,
        }
    }
}
