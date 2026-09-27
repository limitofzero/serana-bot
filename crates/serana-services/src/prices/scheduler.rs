//! The loop that actually sends the digest.
//!
//! A second scheduler beside the reminder one rather than a generalisation of it: they
//! share a shape and nothing else — different store, different port, different recovery —
//! and the reminder loop works. One abstraction over both is worth building when there is a
//! third, not before.

use std::time::Duration;

use serana_domain::prices::{Digest, DigestNotifier, DigestRepository, PriceSource};
use serana_domain::{Clock, NotifyError, StorageError};

use super::config::PriceConfig;

/// What one [`DigestScheduler::tick`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DigestReport {
    pub delivered: usize,
    /// Digests switched off because their recipient is permanently unreachable.
    pub deactivated: usize,
    /// Sends that failed transiently and are left due for the next tick.
    pub retrying: usize,
}

impl DigestReport {
    pub fn is_quiet(&self) -> bool {
        *self == Self::default()
    }
}

/// Sends due digests and advances their schedules.
pub struct DigestScheduler<R, S, N, C> {
    repository: R,
    source: S,
    notifier: N,
    clock: C,
    config: PriceConfig,
}

impl<R, S, N, C> DigestScheduler<R, S, N, C>
where
    R: DigestRepository,
    S: PriceSource,
    N: DigestNotifier,
    C: Clock,
{
    pub fn new(repository: R, source: S, notifier: N, clock: C, config: PriceConfig) -> Self {
        Self {
            repository,
            source,
            notifier,
            clock,
            config,
        }
    }

    /// Send everything due now.
    ///
    /// A digest that came due repeatedly while the process was down is sent **once** and
    /// then rescheduled from the present. A week of downtime should not produce seven
    /// identical messages.
    pub async fn tick(&self) -> Result<DigestReport, StorageError> {
        let now = self.clock.now();
        let mut report = DigestReport::default();

        for mut digest in self.repository.due_at(now).await? {
            let Some(snapshot) = self.read(&digest, now).await else {
                // No prices, so nothing worth sending. Left due, so the next tick tries
                // again — a rate limit that lasts a minute must not cost the day's digest.
                report.retrying += 1;
                continue;
            };

            match self.notifier.notify(digest.owner, &snapshot).await {
                Ok(()) => {
                    digest.mark_sent(now, snapshot)?;
                    self.repository.put(&digest).await?;
                    report.delivered += 1;
                }
                Err(NotifyError::Unreachable(reason)) => {
                    // Retrying forever would mean every tick paying for a delivery that
                    // cannot land. Switch it off and stop.
                    tracing::warn!(
                        owner = %digest.owner,
                        %reason,
                        "switching off the price digest: recipient is unreachable"
                    );
                    digest.next_fire_at = None;
                    // The prices were read, so keep them: asking still answers instantly.
                    digest.cached = Some(snapshot);
                    self.repository.put(&digest).await?;
                    report.deactivated += 1;
                }
                Err(NotifyError::Transport(reason)) => {
                    // Left due, so it is still due on the next tick. What was read is kept
                    // so the retry does not pay for the market twice.
                    tracing::warn!(%reason, "digest delivery failed, will retry");
                    digest.cached = Some(snapshot);
                    self.repository.put(&digest).await?;
                    report.retrying += 1;
                }
            }
        }

        Ok(report)
    }

    /// Prices for this digest, or nothing if the market could not be read.
    async fn read(
        &self,
        digest: &Digest,
        now: jiff::Timestamp,
    ) -> Option<serana_domain::prices::Snapshot> {
        match self.source.quotes(&digest.assets).await {
            Ok(quotes) if !quotes.is_empty() => Some(serana_domain::prices::Snapshot::new(
                quotes,
                now,
                self.source.name(),
            )),
            Ok(_) => {
                tracing::warn!(owner = %digest.owner, "no prices came back for the digest");
                None
            }
            Err(error) => {
                tracing::warn!(%error, "could not read prices for the digest");
                None
            }
        }
    }

    /// Tick forever, `interval` apart.
    pub async fn run(&self, interval: Duration) {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            match self.tick().await {
                Ok(report) if report.is_quiet() => {}
                Ok(report) => tracing::info!(
                    delivered = report.delivered,
                    deactivated = report.deactivated,
                    retrying = report.retrying,
                    "price digest tick"
                ),
                Err(error) => tracing::error!(%error, "price digest tick failed"),
            }
        }
    }

    pub fn config(&self) -> &PriceConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests;
