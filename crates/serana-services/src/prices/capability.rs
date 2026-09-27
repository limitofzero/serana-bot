//! Prices, as a subject a conversation can be about.

use async_trait::async_trait;

use serana_domain::Clock;
use serana_domain::prices::{DigestRepository, PriceSource};
use serana_domain::reminder::{TimeZoneName, UserId};
use serana_domain::tool::ToolSpec;

use crate::chat::Capability;

use super::error::PriceTurnError;
use super::outcome::{PriceOutcome, summarise};
use super::service::PriceService;
use super::{prompt, tools};

/// The prices capability.
pub struct Watching<S, R, C> {
    prices: PriceService<S, R, C>,
}

impl<S, R, C> Watching<S, R, C> {
    pub fn new(prices: PriceService<S, R, C>) -> Self {
        Self { prices }
    }

    pub fn prices(&self) -> &PriceService<S, R, C> {
        &self.prices
    }
}

#[async_trait]
impl<S, R, C> Capability for Watching<S, R, C>
where
    S: PriceSource,
    R: DigestRepository,
    C: Clock,
{
    type Outcome = PriceOutcome;
    type Error = PriceTurnError;

    fn topic(&self) -> &'static str {
        "prices"
    }

    fn instructions(&self) -> &'static str {
        prompt::INSTRUCTIONS
    }

    fn tools(&self) -> Vec<ToolSpec> {
        tools::specs()
    }

    fn knows(&self, name: &str) -> bool {
        tools::is_known(name)
    }

    fn now(&self) -> jiff::Timestamp {
        self.prices.now()
    }

    async fn context(&self, owner: UserId, now: jiff::Timestamp) -> Result<String, PriceTurnError> {
        // Whatever is stored, without creating one: a question about prices should not
        // start a daily push by itself. Showing them does that, deliberately.
        let digest = self.prices.stored(owner).await?;
        let timezone = digest.as_ref().map_or_else(
            || self.prices.config().timezone.clone(),
            |d| d.timezone.clone(),
        );
        let local = now.to_zoned(timezone.resolve()?);
        Ok(prompt::context(&local, &timezone, digest.as_ref(), now))
    }

    async fn run(
        &self,
        owner: UserId,
        _now: jiff::Timestamp,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<PriceOutcome, PriceTurnError> {
        let incomplete = |e| PriceTurnError::Unparsable(format!("the answer was incomplete: {e}"));

        match name {
            tools::SHOW => {
                let args: tools::ShowArgs =
                    serde_json::from_value(arguments).map_err(incomplete)?;
                let (digest, snapshot, read_now) = self.prices.show(owner, args.fresh).await?;
                Ok(PriceOutcome::Showed {
                    digest,
                    snapshot,
                    read_now,
                })
            }
            tools::SCHEDULE => {
                let args: tools::ScheduleArgs =
                    serde_json::from_value(arguments).map_err(incomplete)?;
                let recurrence = args
                    .schedule
                    .into_recurrence()
                    .map_err(PriceTurnError::Unparsable)?;
                let timezone = args
                    .timezone
                    .map(|raw| raw.trim().to_owned())
                    .filter(|raw| !raw.is_empty())
                    .map(TimeZoneName::new);
                Ok(PriceOutcome::Rescheduled(
                    self.prices
                        .set_schedule(owner, recurrence, timezone)
                        .await?,
                ))
            }
            tools::PAUSE => Ok(PriceOutcome::Paused(self.prices.pause(owner).await?)),
            tools::RESUME => Ok(PriceOutcome::Rescheduled(self.prices.resume(owner).await?)),
            tools::ADD => {
                let args: tools::AddArgs = serde_json::from_value(arguments).map_err(incomplete)?;
                let edit = self.prices.add(owner, &args.names).await?;
                let snapshot = if edit.added.is_empty() {
                    edit.digest.cached.clone()
                } else {
                    // The cache was dropped when the list grew, so this reads the market —
                    // and the reply shows the new asset priced, which is what was asked.
                    // A failed read still leaves it added; the prices come next time.
                    match self.prices.show(owner, false).await {
                        Ok((_, snapshot, _)) => Some(snapshot),
                        Err(PriceTurnError::Market(error)) => {
                            tracing::warn!(%error, "added assets, but could not price them");
                            None
                        }
                        Err(other) => return Err(other),
                    }
                };
                Ok(PriceOutcome::Edited { edit, snapshot })
            }
            tools::REMOVE => {
                let args: tools::RemoveArgs =
                    serde_json::from_value(arguments).map_err(incomplete)?;
                let edit = self.prices.remove(owner, &args.names).await?;
                let snapshot = edit.digest.cached.clone();
                Ok(PriceOutcome::Edited { edit, snapshot })
            }
            // `knows` gates the caller, so reaching here means the two lists disagree.
            other => Err(PriceTurnError::Unparsable(format!(
                "I do not know how to {other}"
            ))),
        }
    }

    fn said(&self, words: String) -> PriceOutcome {
        PriceOutcome::Said(words)
    }

    fn summarise(&self, outcome: &PriceOutcome) -> String {
        summarise(outcome)
    }
}
