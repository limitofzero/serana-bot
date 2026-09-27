//! Several price sources, tried in order.
//!
//! A digest that goes out once a day and silently fails is the worst outcome there is: you
//! do not notice for a day, and by then the number you wanted is history. The keyless
//! CoinGecko route shares a rate limit across everyone on the same egress IP, so the
//! failure it has is 429 — transient, total, and not worth losing a day's digest to.
//!
//! This is itself a [`PriceSource`], so nothing above it knows there is more than one.

use async_trait::async_trait;
use serana_domain::prices::{AssetId, PriceError, PriceSource, Quote};

/// Sources in preference order.
pub struct Chain {
    sources: Vec<Box<dyn PriceSource>>,
    /// Which one answered last, so a digest can say when it was not the first choice.
    answered: std::sync::Mutex<Option<String>>,
}

impl Chain {
    /// The first source is the preferred one.
    pub fn new(sources: Vec<Box<dyn PriceSource>>) -> Self {
        Self {
            sources,
            answered: std::sync::Mutex::new(None),
        }
    }

    /// The name of whichever source last answered, or the chain's own if none has yet.
    pub fn answered(&self) -> String {
        self.answered
            .lock()
            .expect("not poisoned")
            .clone()
            .unwrap_or_else(|| self.name().to_owned())
    }
}

#[async_trait]
impl PriceSource for Chain {
    fn name(&self) -> &str {
        self.sources.first().map_or("none", |source| source.name())
    }

    async fn quotes(&self, assets: &[AssetId]) -> Result<Vec<Quote>, PriceError> {
        let mut failures: Vec<String> = Vec::new();

        for source in &self.sources {
            match source.quotes(assets).await {
                // An empty answer is a failure of this source, not an empty market: every
                // asset it was asked about went unpriced, and the next source may know them.
                Ok(quotes) if quotes.is_empty() && !assets.is_empty() => {
                    failures.push(format!("{}: priced nothing", source.name()));
                }
                Ok(quotes) => {
                    *self.answered.lock().expect("not poisoned") = Some(source.name().to_owned());
                    if !failures.is_empty() {
                        tracing::warn!(
                            fell_back_to = source.name(),
                            after = failures.join("; "),
                            "price source fell back"
                        );
                    }
                    return Ok(quotes);
                }
                Err(error) => failures.push(format!("{}: {error}", source.name())),
            }
        }

        Err(PriceError::new(failures.join("; ")))
    }
}
