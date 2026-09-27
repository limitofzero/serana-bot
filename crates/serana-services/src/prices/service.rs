//! The digest lifecycle: reading prices, caching them, and changing when they arrive.
//!
//! No model, no conversation. Everything here is a decision that can be made from a source,
//! a store and a clock — which is what lets all of it be tested without a network.

use serana_domain::Clock;
use serana_domain::prices::{Digest, DigestRepository, PriceSource, Snapshot};
use serana_domain::reminder::{Recurrence, TimeZoneName, UserId};

use super::config::PriceConfig;
use super::error::PriceTurnError;

/// Prices and the standing order for them, independent of how the request arrived.
pub struct PriceService<S, R, C> {
    source: S,
    repository: R,
    clock: C,
    config: PriceConfig,
}

impl<S, R, C> PriceService<S, R, C>
where
    S: PriceSource,
    R: DigestRepository,
    C: Clock,
{
    pub fn new(source: S, repository: R, clock: C, config: PriceConfig) -> Self {
        Self {
            source,
            repository,
            clock,
            config,
        }
    }

    pub fn now(&self) -> jiff::Timestamp {
        self.clock.now()
    }

    pub fn config(&self) -> &PriceConfig {
        &self.config
    }

    /// This person's standing order as it is, without creating one.
    ///
    /// What the prompt reads. Asking a question about prices must not by itself start a
    /// daily push — showing them does that, deliberately.
    pub async fn stored(&self, owner: UserId) -> Result<Option<Digest>, PriceTurnError> {
        Ok(self.repository.get(owner).await?)
    }

    /// This person's standing order, creating it the first time they ask.
    ///
    /// Asking about prices is what starts the daily digest. The alternative — a digest
    /// conjured for everyone at startup — would push a message to someone who never asked
    /// for one, which is the one thing an unprompted daily message must not be.
    pub async fn digest(&self, owner: UserId) -> Result<Digest, PriceTurnError> {
        if let Some(digest) = self.repository.get(owner).await? {
            return Ok(digest);
        }
        let mut digest = Digest::new(
            owner,
            self.config.recurrence.clone(),
            self.config.timezone.clone(),
            self.config.assets.clone(),
        );
        digest.reschedule(self.now())?;
        self.repository.put(&digest).await?;
        Ok(digest)
    }

    /// The prices, read again only when they have to be.
    ///
    /// Returns whether they were read for this reply. That is the whole bargain of a
    /// digest: the scheduled run pays for the request, and asking afterwards costs nothing.
    pub async fn show(
        &self,
        owner: UserId,
        fresh: bool,
    ) -> Result<(Digest, Snapshot, bool), PriceTurnError> {
        let now = self.now();
        let mut digest = self.digest(owner).await?;

        if !fresh && !digest.is_stale(now, self.config.cache_ttl) {
            let cached = digest
                .cached
                .clone()
                .expect("not stale, so there is something cached");
            return Ok((digest, cached, false));
        }

        match self.read(&digest, now).await {
            Ok(snapshot) => {
                digest.cached = Some(snapshot.clone());
                self.repository.put(&digest).await?;
                Ok((digest, snapshot, true))
            }
            // What is cached is better than nothing, even when it is old — as long as the
            // reply says how old. Only a digest with nothing at all to show fails.
            Err(error) => match digest.cached.clone() {
                Some(cached) => {
                    tracing::warn!(%error, "showing cached prices: the market could not be read");
                    Ok((digest, cached, false))
                }
                None => Err(error),
            },
        }
    }

    /// Read prices now, without touching what is stored.
    pub async fn read(
        &self,
        digest: &Digest,
        now: jiff::Timestamp,
    ) -> Result<Snapshot, PriceTurnError> {
        let quotes = self.source.quotes(&digest.assets).await?;
        Ok(Snapshot::new(quotes, now, self.source.name()))
    }

    /// Change when the digest arrives, and optionally which zone it is read in.
    pub async fn set_schedule(
        &self,
        owner: UserId,
        recurrence: Recurrence,
        timezone: Option<TimeZoneName>,
    ) -> Result<Digest, PriceTurnError> {
        let mut digest = self.digest(owner).await?;
        if let Some(timezone) = timezone {
            // Checked before it is stored: a zone that will not resolve would turn every
            // later reschedule into a failure, far from the message that caused it.
            timezone.resolve()?;
            digest.timezone = timezone;
        }
        digest.recurrence = recurrence;
        if digest.reschedule(self.now())?.is_none() {
            return Err(PriceTurnError::Unparsable(
                "that schedule has no next occurrence — check the date".into(),
            ));
        }
        self.repository.put(&digest).await?;
        Ok(digest)
    }

    /// Stop the digest arriving. What is cached stays, so asking still answers instantly.
    pub async fn pause(&self, owner: UserId) -> Result<Digest, PriceTurnError> {
        let mut digest = self.digest(owner).await?;
        digest.next_fire_at = None;
        self.repository.put(&digest).await?;
        Ok(digest)
    }

    /// Bring the digest back on the schedule it already has.
    pub async fn resume(&self, owner: UserId) -> Result<Digest, PriceTurnError> {
        let mut digest = self.digest(owner).await?;
        digest.reschedule(self.now())?;
        self.repository.put(&digest).await?;
        Ok(digest)
    }
}

#[cfg(test)]
mod tests;
