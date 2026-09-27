//! The digest lifecycle: reading prices, caching them, and changing when they arrive.
//!
//! No model, no conversation. Everything here is a decision that can be made from a source,
//! a store and a clock — which is what lets all of it be tested without a network.

use serana_domain::Clock;
use serana_domain::prices::{
    AssetMatch, Digest, DigestRepository, PriceSource, Snapshot, best_match, search_terms,
};
use serana_domain::reminder::{Recurrence, TimeZoneName, UserId};

use super::config::PriceConfig;
use super::error::PriceTurnError;
use super::outcome::WatchlistEdit;

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

    /// Look each name up and watch what it resolves to.
    ///
    /// The model passes names as the person said them and never an id: ids are exactly what
    /// a model invents plausibly and wrongly, and the source's own search knows them. Every
    /// lookup happens before anything changes, so a rate limit on the third name does not
    /// leave the list half-edited.
    pub async fn add(
        &self,
        owner: UserId,
        names: &[String],
    ) -> Result<WatchlistEdit, PriceTurnError> {
        let mut resolved: Vec<AssetMatch> = Vec::new();
        let mut missing: Vec<String> = Vec::new();
        for name in names {
            let Some(terms) = search_terms(name) else {
                continue;
            };
            let found = self.source.search(&terms).await?;
            match best_match(&terms, &found) {
                Some(chosen) => resolved.push(chosen.clone()),
                None => missing.push(terms),
            }
        }
        if resolved.is_empty() && missing.is_empty() {
            return Err(PriceTurnError::Unparsable("say which asset to add".into()));
        }

        let mut digest = self.digest(owner).await?;
        let (mut added, mut already) = (Vec::new(), Vec::new());
        for asset in resolved {
            if digest.watch(asset.id.clone()) {
                added.push(asset);
            } else {
                already.push(asset);
            }
        }
        if !added.is_empty() {
            self.repository.put(&digest).await?;
        }
        Ok(WatchlistEdit {
            digest,
            added,
            already,
            removed: Vec::new(),
            missing,
        })
    }

    /// Stop watching each named asset — by ticker or by id, whichever they used.
    ///
    /// Refuses to empty the list. A digest of nothing would be retried every tick forever,
    /// and "stop sending it" is what someone who wants none of it means; that is what
    /// [`Self::pause`] is for.
    pub async fn remove(
        &self,
        owner: UserId,
        names: &[String],
    ) -> Result<WatchlistEdit, PriceTurnError> {
        let mut digest = self.digest(owner).await?;
        let mut targets = Vec::new();
        let mut missing = Vec::new();
        for name in names {
            let wanted = search_terms(name).unwrap_or_else(|| name.trim().to_owned());
            match digest.find(&wanted).or_else(|| digest.find(name)) {
                Some(id) if !targets.contains(id) => targets.push(id.clone()),
                Some(_) => {}
                None if !wanted.is_empty() => missing.push(wanted),
                None => {}
            }
        }
        if !targets.is_empty() && targets.len() >= digest.assets.len() {
            return Err(PriceTurnError::Unparsable(
                "that would leave nothing to watch — say \"stop sending it\" to switch the \
                 digest off instead"
                    .into(),
            ));
        }

        let mut removed = Vec::new();
        for id in &targets {
            removed.push(label(&digest, id));
            digest.unwatch(id);
        }
        if !removed.is_empty() {
            self.repository.put(&digest).await?;
        }
        Ok(WatchlistEdit {
            digest,
            added: Vec::new(),
            already: Vec::new(),
            removed,
            missing,
        })
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

/// How a watched asset was last shown: its ticker if it has been priced, its id otherwise.
fn label(digest: &Digest, id: &serana_domain::prices::AssetId) -> String {
    digest
        .cached
        .as_ref()
        .and_then(|snapshot| snapshot.quotes.iter().find(|quote| &quote.asset == id))
        .map_or_else(|| id.to_string(), |quote| quote.symbol.clone())
}

#[cfg(test)]
mod tests;
